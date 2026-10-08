//! RT7 model geometry (renderer plan M10): the static locs drawn from
//! their RT7 models (archive 47, `rs910_config::nxt::model_rt7`) instead of
//! the classic meshes, CPU only (the draw goes through the forward pipeline's
//! ordinary model path, `crate::frame::resources`).
//!
//! # What an RT7 model is (proven, `nxt-data-formats.md` §9)
//!
//! The modern client's re-export of the classic model with the same id: the
//! same faces at the same positions (y negated, times 4 below classic version
//! 13), the same face colours, alphas, materials and labels, grouped into
//! meshes by material and priority, with split vertices carrying precomputed
//! normals, tangents and UVs, and LOD index lists. The modern client loads
//! every model from archive 47 (archive 7 is never read) and bakes the face
//! HSL colours into the vertices.
//!
//! # What this backend draws
//!
//! For a static loc (scenery, wall, wall decor, ground decor; not a
//! dynamic loc, not a temporary or player) whose snapshot model is the
//! loc's shape models built the classic way: the RT7 mesh
//! with
//!
//! - **positions** taken from the snapshot's classic model (its vertex x/y/z
//!   arrays, every transform the classic build applied: mirror, rotation,
//!   scale, offsets, `hillchange` contouring, post offsets), through the correspondence
//!   RT7 vertex -> classic source vertex (equal raw positions), so the RT7
//!   geometry sits exactly where the faithful mesh does;
//! - **colour, alpha and material** of the corresponding classic face (RT7
//!   face -> classic face by its three raw positions; the classic recolour,
//!   retexture, tint and ambient bake, `GpuModel::colour_stream`, the
//!   colours the modern look is calibrated on);
//! - **normals** from RT7 (classic axes: y negated), carried through the
//!   linear part of the model's placement (least-squares fit of the classic
//!   raw -> final positions; accepted when every vertex fits within
//!   [`RIGID_TOLERANCE`], i.e. the placement is affine; a contoured or
//!   posed model keeps the classic mesh), **UVs** from RT7, tangents from the
//!   UVs (`crate::models::mesh::add_tangents`, the M2 convention);
//! - LOD 0, the non-hidden meshes (the modern client draws no section for
//!   hidden ones), triangles wound as the classic faces (RT7 `(a, b, c)` is
//!   classic `(f1, f3, f2)`).
//!
//! The draw list, keys, order, shadows, billboards and lights are the classic
//! entity's; only the vertex streams differ. An entity whose models or
//! faces do not correspond one to one (a customised or morphed loc, a
//! model the classic build changed) keeps its classic mesh ([`Rt7Stats`] counts why).
//!
//! Animated models (dynamic locs, NPCs, players, spot anims) draw their
//! RT7 meshes posed as their classic models through [`crate::models::rt7_anim`].

use std::collections::HashMap;
use std::sync::Arc;

use rs910_config::nxt::model_rt7::{decode_model_rt7, Rt7Model, MODEL_RT7_ARCHIVE};

use crate::gpumodel::GpuModel;
use crate::models::draw_list::{EntityDraw, Kind};
use crate::models::mesh::{ModelStreams, Vertex};
use crate::scene::EntityRef;
use crate::scene_snapshot::SceneSnapshot;
use crate::texture::MaterialStore;

/// Largest distance (fine units) between a classic vertex and the affine fit
/// of the model's placement for the placement to count as rigid.
pub const RIGID_TOLERANCE: f32 = 1.5;

/// Why an entity kept its classic mesh, and how many drew RT7.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rt7Stats {
    pub drawn: usize,
    /// Drawn with a contoured placement (positions the classic ones,
    /// normals the rigid part's).
    pub contoured: usize,
    pub faces: usize,
    /// Not a static loc (or a dynamic one).
    pub not_static: usize,
    pub no_config: usize,
    pub no_rt7: usize,
    /// The classic model is not the raw shape models (vertex counts differ).
    pub other_model: usize,
    /// An RT7 vertex or face without a classic counterpart, or classic faces
    /// left over.
    pub unmatched: usize,
    /// The placement is not affine (contoured, posed).
    pub non_rigid: usize,
    /// More vertices than 16-bit indices address.
    pub too_large: usize,
}

/// The per-renderer RT7 state: the loc configs, decoded RT7 models and raw
/// classic models by id.
#[derive(Default)]
pub struct Rt7Cache {
    /// The loc configs (prepare-only: the store is not `Sync`).
    locs: Option<Option<rs910_config::config::LocStore>>,
    models: HashMap<i32, Option<Arc<Rt7Model>>>,
    raw: HashMap<i32, Option<Arc<crate::modelunlit::ModelUnlit>>>,
    pub stats: Rt7Stats,
    /// The triangles of each entity drawn from RT7 (the check).
    drawn: HashMap<crate::scene_snapshot::EntityKey, usize>,
    /// CPU time spent building RT7 streams (the log and the cost test).
    pub build_time: std::time::Duration,
    /// The animated models' RT7 maps ([`crate::models::rt7_anim`]).
    pub anim: crate::models::rt7_anim::AnimCache,
}

/// Raw (model-space, classic axes) position key.
type Key = [i32; 3];

impl Rt7Cache {
    /// The posed RT7 streams of animated `entity`
    /// ([`crate::models::rt7_anim::AnimCache::animated`] over this cache's
    /// loaders), `None` to keep its classic mesh; off with the RT7 models.
    pub fn animated(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        materials: &MaterialStore,
        entity: &EntityDraw<'_>,
        frame: Option<u64>,
    ) -> Option<ModelStreams> {
        let Self {
            anim, models, raw, ..
        } = self;
        anim.animated(snapshot, materials, entity, frame, &mut |pack, id| {
            let id = i32::try_from(id).ok()?;
            Some((load_raw(raw, pack, id)?, load_rt7(models, pack, id)?))
        })
    }

    /// The map animated `entity` poses with
    /// ([`crate::models::rt7_anim::AnimCache::map_for`] over this cache's
    /// loaders): the first half of [`Self::animated`], whose pose the
    /// renderer's threads run (`frame::posing`).
    pub fn anim_map(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
        frame: Option<u64>,
    ) -> Option<Arc<crate::models::rt7_anim::AnimMap>> {
        let Self {
            anim, models, raw, ..
        } = self;
        anim.map_for(snapshot, entity, frame, &mut |pack, id| {
            let id = i32::try_from(id).ok()?;
            Some((load_raw(raw, pack, id)?, load_rt7(models, pack, id)?))
        })
    }

    /// Whether the snapshot's keyed loc has a dynamic pose. This reads
    /// only scene ownership, before any static model decoding.
    pub(crate) fn is_dynamic(snapshot: &SceneSnapshot<'_>, entity: &EntityDraw<'_>) -> bool {
        if snapshot
            .live_frame()
            .is_some_and(|live| live.has_dynamic(entity.id))
        {
            return true;
        }
        let Some(scene) = snapshot.scene else {
            return false;
        };
        match entity.key.map(|k| k.source) {
            Some(EntityRef::Scenery(i)) => scene.scenery.get(i).is_some_and(|e| e.dynamic),
            Some(EntityRef::Wall(i)) => scene.walls.get(i).is_some_and(|e| e.dynamic),
            Some(EntityRef::WallDecor(i)) => scene.wall_decors.get(i).is_some_and(|e| e.dynamic),
            Some(EntityRef::GroundDecor(i)) => {
                scene.ground_decors.get(i).is_some_and(|e| e.dynamic)
            }
            _ => false,
        }
    }

    /// RT7 model `id`, decoded once (tests).
    #[cfg(test)]
    pub(crate) fn rt7_model(
        &mut self,
        pack: &crate::cache::Pack,
        id: i32,
    ) -> Option<Arc<Rt7Model>> {
        load_rt7(&mut self.models, pack, id)
    }

    /// The classic model `id` as the loc loader makes it, once (tests).
    #[cfg(test)]
    pub(crate) fn raw_model(
        &mut self,
        pack: &crate::cache::Pack,
        id: i32,
    ) -> Option<Arc<crate::modelunlit::ModelUnlit>> {
        load_raw(&mut self.raw, pack, id)
    }

    /// The RT7 streams of `entity` (see the module docs), `None` to keep
    /// its classic mesh: [`Self::plan`], [`Rt7Plan::build`] and
    /// [`Self::count`] in turn (the renderer's threads run the middle step
    /// for a batch of locs ahead of their draws, `frame::prebuild`).
    pub fn streams(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        materials: &MaterialStore,
        entity: &EntityDraw<'_>,
    ) -> Option<ModelStreams> {
        let start = std::time::Instant::now();
        match self.plan(snapshot, entity) {
            Rt7Plan::Skip => None,
            Rt7Plan::NotStatic => {
                self.stats.not_static += 1;
                None
            }
            Rt7Plan::Dynamic => {
                self.stats.not_static += 1;
                // A dynamic loc's RT7 mesh posed as its classic model.
                self.animated(snapshot, materials, entity, None)
            }
            plan => {
                let outcome = plan.build(entity.model, materials);
                self.build_time += start.elapsed();
                self.count(entity, outcome)
            }
        }
    }

    /// What `entity`'s RT7 streams are built from: its loc's shape models,
    /// decoded here (once per model) on the calling thread, or why it has
    /// none. Counts nothing ([`Self::count`] does, with the outcome).
    pub(crate) fn plan(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
    ) -> Rt7Plan {
        let (pack, ids, y_scale) = match self.shape_models(snapshot, entity) {
            Ok(found) => found,
            Err(plan) => return plan,
        };
        let mut parts = Vec::with_capacity(ids.len());
        let mut raws = Vec::with_capacity(ids.len());
        for &id in &ids {
            let (Some(rt7), Some(raw)) = (
                load_rt7(&mut self.models, pack, id),
                load_raw(&mut self.raw, pack, id),
            ) else {
                return Rt7Plan::NoRt7;
            };
            parts.push(rt7);
            raws.push(raw);
        }
        Rt7Plan::Build {
            raws,
            parts,
            y_scale,
        }
    }

    /// The shape models of `entity`'s plan not decoded yet (to decode ahead
    /// on other threads with [`decode_models`] and [`Self::insert_models`]).
    pub(crate) fn missing_models(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        entity: &EntityDraw<'_>,
    ) -> Vec<i32> {
        match self.shape_models(snapshot, entity) {
            Ok((_, ids, _)) => ids
                .into_iter()
                .filter(|id| !self.models.contains_key(id) || !self.raw.contains_key(id))
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Cache the shape models [`decode_models`] decoded for `ids` (a model
    /// decoded meanwhile keeps its entry).
    pub(crate) fn insert_models(&mut self, ids: &[i32], decoded: Vec<DecodedModels>) {
        for (&id, (rt7, raw)) in ids.iter().zip(decoded) {
            self.models.entry(id).or_insert(rt7);
            self.raw.entry(id).or_insert(raw);
        }
    }

    /// `entity`'s pack, shape model ids and `resizey` scale, or its plan when
    /// it has none (not a static loc, dynamic, no config).
    fn shape_models<'p>(
        &mut self,
        snapshot: &SceneSnapshot<'p>,
        entity: &EntityDraw<'_>,
    ) -> Result<(&'p crate::cache::Pack, Vec<i32>, f32), Rt7Plan> {
        let (Some(pack), Some(scene)) = (snapshot.pack, snapshot.scene) else {
            return Err(Rt7Plan::Skip);
        };
        let dynamic = Self::is_dynamic(snapshot, entity);
        let found = match (entity.kind, entity.key.map(|k| k.source)) {
            (Kind::Model, Some(EntityRef::Scenery(i))) => {
                scene.scenery.get(i).map(|e| (e.loc_id, e.shape, e.dynamic))
            }
            (Kind::Model, Some(EntityRef::Wall(i))) => {
                scene.walls.get(i).map(|e| (e.loc_id, e.shape, e.dynamic))
            }
            (Kind::Model, Some(EntityRef::WallDecor(i))) => scene
                .wall_decors
                .get(i)
                .map(|e| (e.loc_id, e.shape, e.dynamic)),
            (Kind::Model, Some(EntityRef::GroundDecor(i))) => scene.ground_decors.get(i).map(|e| {
                (
                    e.loc_id,
                    rs910_scene::loctype::shape::GROUND_DECOR,
                    e.dynamic,
                )
            }),
            _ => return Err(Rt7Plan::NotStatic),
        };
        let Some((loc_id, shape, is_dynamic)) = found else {
            return Err(Rt7Plan::Skip);
        };
        if dynamic || is_dynamic {
            return Err(Rt7Plan::Dynamic);
        }
        let shape = if rs910_scene::loctype::shape::is_wall_decor(shape) {
            rs910_scene::loctype::shape::WALLDECOR_STRAIGHT_NOOFFSET
        } else {
            shape
        };
        if self.locs.is_none() {
            self.locs = Some(rs910_config::config::LocStore::load(pack).ok());
        }
        let locs = self.locs.as_ref().and_then(Option::as_ref);
        let loc = locs.and_then(|l| l.get(loc_id));
        let Some(ids) = loc
            .and_then(|l| l.shape_models.iter().find(|(s, _)| i32::from(*s) == shape))
            .map(|(_, ids)| ids.clone())
            .filter(|ids| !ids.is_empty())
        else {
            return Err(Rt7Plan::NoConfig);
        };
        let y_scale = loc.map_or(1.0, |l| l.resizey as f32 / 128.0);
        Ok((pack, ids, y_scale))
    }

    /// Count `outcome` (a [`Rt7Plan::build`] of `entity`'s plan) as
    /// [`Self::streams`] does and return its streams.
    pub(crate) fn count(
        &mut self,
        entity: &EntityDraw<'_>,
        outcome: Rt7Outcome,
    ) -> Option<ModelStreams> {
        let out = match outcome {
            Rt7Outcome::NoConfig => {
                self.stats.no_config += 1;
                None
            }
            Rt7Outcome::NoRt7 => {
                self.stats.no_rt7 += 1;
                None
            }
            Rt7Outcome::Built(streams, contoured) => {
                self.stats.drawn += 1;
                self.stats.contoured += usize::from(contoured);
                self.stats.faces += streams.indices.len() / 3;
                Some(streams)
            }
            Rt7Outcome::Rejected(why) => {
                match why {
                    Reject::OtherModel => self.stats.other_model += 1,
                    Reject::Unmatched => self.stats.unmatched += 1,
                    Reject::NonRigid => self.stats.non_rigid += 1,
                    Reject::TooLarge => self.stats.too_large += 1,
                }
                None
            }
        };
        if let Some(key) = entity.key {
            match &out {
                Some(s) => self.drawn.insert(key, s.indices.len() / 3),
                None => self.drawn.remove(&key),
            };
        }
        out
    }

    /// `CLIENT910_MODERN_CHECK` (M10): every entity of the frame drawn from
    /// RT7 draws one triangle per face its classic model draws (all but the
    /// one-point faces RT7 leaves out). Returns `(entities, mismatches)`.
    #[must_use]
    pub fn check(&self, entities: &[EntityDraw<'_>]) -> (usize, usize) {
        let (mut n, mut bad) = (0, 0);
        for e in entities {
            let Some(&tris) = e.key.and_then(|k| self.drawn.get(&k)) else {
                continue;
            };
            n += 1;
            bad += usize::from(tris != classic_draw_faces(e.model));
        }
        (n, bad)
    }
}

/// What a loc's RT7 streams are built from ([`Rt7Cache::plan`]).
pub(crate) enum Rt7Plan {
    /// No pack or scene, or no scene entity: nothing counted, no streams.
    Skip,
    /// Not a static loc kind.
    NotStatic,
    /// A dynamic loc: posed ([`Rt7Cache::animated`], on the calling thread).
    Dynamic,
    NoConfig,
    NoRt7,
    /// The loc's shape models (classic raw and RT7) and its `resizey` scale.
    Build {
        raws: Vec<Arc<crate::modelunlit::ModelUnlit>>,
        parts: Vec<Arc<Rt7Model>>,
        y_scale: f32,
    },
}

/// A built [`Rt7Plan`] ([`Rt7Plan::build`]), counted by [`Rt7Cache::count`].
pub(crate) enum Rt7Outcome {
    NoConfig,
    NoRt7,
    Built(ModelStreams, bool),
    Rejected(Reject),
}

impl Rt7Plan {
    /// Build the streams of classic model `model` from this plan: no cache or
    /// counter is touched, so any thread may run it. `Skip`, `NotStatic`
    /// and `Dynamic` plans are not built here (they give `NoConfig`, which
    /// callers never ask for).
    pub(crate) fn build(&self, model: &GpuModel, materials: &MaterialStore) -> Rt7Outcome {
        match self {
            Self::Build {
                raws,
                parts,
                y_scale,
            } => {
                let merged;
                let raw: &crate::modelunlit::ModelUnlit = if raws.len() == 1 {
                    &raws[0]
                } else {
                    let refs: Vec<&crate::modelunlit::ModelUnlit> =
                        raws.iter().map(|r| r.as_ref()).collect();
                    merged = crate::modelunlit::ModelUnlit::merge(&refs);
                    &merged
                };
                match build(model, raw, parts, materials, *y_scale) {
                    Ok((streams, contoured)) => Rt7Outcome::Built(streams, contoured),
                    Err(why) => Rt7Outcome::Rejected(why),
                }
            }
            Self::NoRt7 => Rt7Outcome::NoRt7,
            Self::NoConfig | Self::Skip | Self::NotStatic | Self::Dynamic => Rt7Outcome::NoConfig,
        }
    }

    /// Whether [`Self::build`] can run off the calling thread (everything but
    /// the skipped, non-static and dynamic locs).
    pub(crate) fn buildable(&self) -> bool {
        matches!(self, Self::Build { .. } | Self::NoConfig | Self::NoRt7)
    }
}

/// RT7 model `id` from `cache`, decoded once.
fn load_rt7(
    cache: &mut HashMap<i32, Option<Arc<Rt7Model>>>,
    pack: &crate::cache::Pack,
    id: i32,
) -> Option<Arc<Rt7Model>> {
    cache
        .entry(id)
        .or_insert_with(|| decode_rt7(pack, id))
        .clone()
}

/// RT7 model `id` decoded from `pack`.
fn decode_rt7(pack: &crate::cache::Pack, id: i32) -> Option<Arc<Rt7Model>> {
    let group = u32::try_from(id).ok()?;
    let files = pack.read_group(MODEL_RT7_ARCHIVE, group).ok()?;
    match decode_model_rt7(group, files.get(&0)?) {
        Ok(m) => Some(Arc::new(m)),
        Err(err) => {
            log::warn!("[modern] rt7 model {id}: {err:#}");
            None
        }
    }
}

/// The classic model `id` from `cache` as the loc loader makes it, once.
fn load_raw(
    cache: &mut HashMap<i32, Option<Arc<crate::modelunlit::ModelUnlit>>>,
    pack: &crate::cache::Pack,
    id: i32,
) -> Option<Arc<crate::modelunlit::ModelUnlit>> {
    cache
        .entry(id)
        .or_insert_with(|| decode_raw(pack, id))
        .clone()
}

/// The classic model `id` decoded from `pack` as the loc loader makes it: times 4
/// below version 13.
fn decode_raw(pack: &crate::cache::Pack, id: i32) -> Option<Arc<crate::modelunlit::ModelUnlit>> {
    let mut m = crate::modelunlit::ModelUnlit::load(pack, u32::try_from(id).ok()?).ok()?;
    if m.version < 13 {
        m.scale_by_power_of_two(2);
    }
    Some(Arc::new(m))
}

/// A shape model's RT7 and classic copies ([`decode_models`]).
pub(crate) type DecodedModels = (
    Option<Arc<Rt7Model>>,
    Option<Arc<crate::modelunlit::ModelUnlit>>,
);

/// Shape model `id`'s RT7 and classic copies, decoded from `pack` (no cache
/// touched: any thread may run it; [`Rt7Cache::insert_models`] keeps them).
pub(crate) fn decode_models(pack: &crate::cache::Pack, id: i32) -> DecodedModels {
    (decode_rt7(pack, id), decode_raw(pack, id))
}

/// The faces `model` draws that RT7 has (all but the one-point faces).
#[must_use]
pub fn classic_draw_faces(model: &GpuModel) -> usize {
    (0..model.draw_face_count as usize)
        .filter(|&f| {
            let s = [model.idx1[f], model.idx2[f], model.idx3[f]]
                .map(|u| usize::from(model.unique_vertex[usize::from(u as u16)] as u16));
            let p = s.map(|i| [model.vx[i], model.vy[i], model.vz[i]]);
            !(p[0] == p[1] && p[1] == p[2])
        })
        .count()
}

/// Why [`build`] kept the classic mesh.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reject {
    OtherModel,
    Unmatched,
    NonRigid,
    TooLarge,
}

/// The affine fit `final = a * raw + t` of the model's placement
/// (least squares over the vertices, a flat or tiny model's missing axes
/// regularised towards zero), and its largest residual per axis.
#[must_use]
pub fn fit_affine(
    pairs: &[(glam::Vec3, glam::Vec3)],
) -> Option<(glam::Mat3, glam::Vec3, glam::Vec3)> {
    if pairs.is_empty() {
        return None;
    }
    // Normal equations over [x y z 1] (f64 for the conditioning), centred.
    let n = pairs.len() as f64;
    let mean = |f: &dyn Fn(&(glam::Vec3, glam::Vec3)) -> glam::DVec3| {
        pairs.iter().map(f).fold(glam::DVec3::ZERO, |a, b| a + b) / n
    };
    let cr = mean(&|p| p.0.as_dvec3());
    let cf = mean(&|p| p.1.as_dvec3());
    let mut m = glam::DMat3::ZERO;
    let mut b = glam::DMat3::ZERO;
    for (r, f) in pairs {
        let r = r.as_dvec3() - cr;
        let f = f.as_dvec3() - cf;
        // m += r r^T, b += f r^T
        m += glam::DMat3::from_cols(r * r.x, r * r.y, r * r.z);
        b += glam::DMat3::from_cols(f * r.x, f * r.y, f * r.z);
    }
    if m.determinant().abs() < 1e-6 {
        // Flat models (a single plane): regularise the missing axis.
        m += glam::DMat3::IDENTITY * 1e-3;
    }
    let a = b * m.inverse();
    let t = cf - a * cr;
    let a32 = a.as_mat3();
    let t32 = t.as_vec3();
    let worst = pairs
        .iter()
        .map(|(r, f)| (a32 * *r + t32 - *f).abs())
        .fold(glam::Vec3::ZERO, glam::Vec3::max);
    Some((a32, t32, worst))
}

/// Build the RT7 streams of classic model `model` (its raw shape models
/// merged: `raw`; their RT7 copies: `parts`).
pub fn build(
    model: &GpuModel,
    raw: &crate::modelunlit::ModelUnlit,
    parts: &[Arc<Rt7Model>],
    materials: &MaterialStore,
    y_scale: f32,
) -> Result<(ModelStreams, bool), Reject> {
    // The classic model keeps the vertices up to the last one a face uses
    // (`used_vertex_count`).
    let count = model.vertex_count as usize;
    if count > raw.vertex_count as usize
        || model.vx.len() < count
        || count < raw.used_vertex_count as usize
    {
        return Err(Reject::OtherModel);
    }
    let raw_pos = |i: usize| -> Key { [raw.vertex_x[i], raw.vertex_y[i], raw.vertex_z[i]] };
    // Raw position -> first source vertex.
    let mut by_pos: HashMap<Key, usize> = HashMap::new();
    for i in 0..count {
        by_pos.entry(raw_pos(i)).or_insert(i);
    }
    // classic draw face -> its three raw positions (sorted).
    let colours = model
        .colour_stream(materials)
        .map_err(|_| Reject::OtherModel)?;
    let mut faces: HashMap<[Key; 3], Vec<usize>> = HashMap::new();
    let mut classic_faces = 0;
    for f in 0..model.draw_face_count as usize {
        let src = [model.idx1[f], model.idx2[f], model.idx3[f]]
            .map(|u| usize::from(model.unique_vertex[usize::from(u as u16)] as u16));
        let mut key = src.map(raw_pos);
        // RT7 leaves out only the faces whose corners are one point
        // (`rt7_models_are_the_classic_models`).
        if key[0] == key[1] && key[1] == key[2] {
            continue;
        }
        key.sort_unstable();
        classic_faces += 1;
        faces.entry(key).or_default().push(f);
    }
    // The placement's linear part.
    let pairs: Vec<(glam::Vec3, glam::Vec3)> = (0..count)
        .map(|i| {
            let r = raw_pos(i);
            (
                glam::Vec3::new(r[0] as f32, r[1] as f32, r[2] as f32),
                glam::Vec3::new(model.vx[i] as f32, model.vy[i] as f32, model.vz[i] as f32),
            )
        })
        .collect();
    let Some((mut linear, _, worst)) = fit_affine(&pairs) else {
        return Err(Reject::NonRigid);
    };
    let mut contoured = false;
    if worst.x > RIGID_TOLERANCE || worst.z > RIGID_TOLERANCE {
        return Err(Reject::NonRigid);
    }
    if worst.y > RIGID_TOLERANCE {
        // Contoured (`hillchange`, which moves y only): the rotation,
        // mirror and scale of x and z from the fit, the vertical scale
        // `y_scale`; the normals ignore the drape.
        let (r0, r2) = (linear.row(0), linear.row(2));
        linear = glam::Mat3::from_cols(
            glam::Vec3::new(r0.x, 0.0, r2.x),
            glam::Vec3::new(r0.y, y_scale, r2.y),
            glam::Vec3::new(r0.z, 0.0, r2.z),
        );
        contoured = true;
    }
    let normal_matrix = linear.inverse().transpose();
    let mut out = ModelStreams::default();
    let mut used = vec![false; model.draw_face_count as usize];
    // (material, indices) groups in RT7 mesh order.
    let mut groups: Vec<(i32, Vec<u32>)> = Vec::new();
    for part in parts {
        for mesh in &part.meshes {
            let Some(lod0) = mesh.lods.first() else {
                continue;
            };
            let mut local: Vec<Option<u32>> = vec![None; mesh.positions.len()];
            for tri in lod0.chunks_exact(3) {
                let corners = [tri[0], tri[1], tri[2]].map(usize::from);
                let keys = corners.map(|c| {
                    let p = mesh.positions[c];
                    [i32::from(p[0]), -i32::from(p[1]), i32::from(p[2])]
                });
                let mut sorted = keys;
                sorted.sort_unstable();
                // The first unused classic face on these positions (classic
                // models repeat some faces). An RT7 face without one is a
                // face the classic model does not draw (`draw_face_count` leaves out
                // its hidden faces): not drawn either.
                let Some(&f) = faces
                    .get(&sorted)
                    .and_then(|list| list.iter().find(|&&f| !used[f]))
                else {
                    continue;
                };
                used[f] = true;
                if mesh.hidden() {
                    // Matched (its classic face is a billboard's or an
                    // emitter's), not drawn.
                    continue;
                }
                // Face indices are stored as `i16` holding unsigned 16-bit
                // values: a model past 32767 unique vertices has negative
                // ones, read unsigned like everywhere else (the index stream,
                // `unique_vertex` above).
                let colour = colours[usize::from(model.idx1[f] as u16)] as u32;
                let material = i32::from(model.face_material[f]);
                let mut idx = [0_u32; 3];
                for (k, &c) in corners.iter().enumerate() {
                    let src = *by_pos.get(&keys[k]).ok_or(Reject::Unmatched)?;
                    let slot = match local[c] {
                        // A vertex shared by faces of different colours
                        // would lose one; the 910 data has none (the
                        // research census), but be safe.
                        Some(v) if out.colours[v as usize] == colour => v,
                        _ => {
                            let n = mesh.normals[c];
                            let n = normal_matrix
                                * glam::Vec3::new(
                                    f32::from(n[0]),
                                    -f32::from(n[1]),
                                    f32::from(n[2]),
                                );
                            let v = out.vertices.len() as u32;
                            out.vertices.push(Vertex {
                                pos: [
                                    model.vx[src] as f32,
                                    model.vy[src] as f32,
                                    model.vz[src] as f32,
                                ],
                                normal: n.normalize_or_zero().to_array(),
                                uv: mesh.uvs[c],
                                tangent: [0.0; 4],
                            });
                            out.colours.push(colour);
                            local[c] = Some(v);
                            v
                        }
                    };
                    idx[k] = slot;
                }
                // The classic winding: RT7 (a, b, c) is classic (f1, f3, f2).
                let tri = [idx[0], idx[2], idx[1]];
                match groups.last_mut() {
                    Some((m, list)) if *m == material => list.extend_from_slice(&tri),
                    _ => groups.push((material, tri.to_vec())),
                }
            }
        }
    }
    if out.vertices.len() > usize::from(u16::MAX) {
        return Err(Reject::TooLarge);
    }
    if used.iter().filter(|&&u| u).count() != classic_faces {
        return Err(Reject::Unmatched);
    }
    for (material, list) in groups {
        let first = out.indices.len() as u32;
        out.indices.extend(list.iter().map(|&i| i as u16));
        out.batches.push((material, first, list.len() as u32));
    }
    crate::models::mesh::add_tangents(&mut out.vertices, &out.indices);
    Ok((out, contoured))
}
