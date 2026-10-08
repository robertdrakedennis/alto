//! The far scene's build jobs (milestones F3 and F5): what one worker (or,
//! in sync mode, the render thread) builds for a map square or a near
//! extension container, CPU only. The GPU half uploads the results
//! (`frame::gpu::far`).
//!
//! - [`terrain`]: a square's terrain (`crate::terrain::build_square` over
//!   the shared decode cache), its materials numbered locally (the render
//!   thread maps them onto the far texture array when it uploads).
//! - [`LocWorker::square`]: a square's far locs, placed privately
//!   (`rs910_far_scene::far_locs::place_square`), each built as RT7 geometry
//!   with its LOD lists ([`crate::far::lod`]; the classic mesh where RT7 does
//!   not correspond), merged per 16 x 16-tile container
//!   ([`crate::far::batch`]).
//! - [`LocWorker::extension`]: one container of the classic window's own
//!   static locs (the near extension) from slim copies of their snapshot
//!   models, LOD 0 as the near path draws them.
//!
//! Every read is resident-only (`Pack::read_group_resident`); the caches
//! are the workers' own.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use rs910_config::nxt::model_rt7::{decode_model_rt7, Rt7Model, MODEL_RT7_ARCHIVE};
use rs910_far_scene::far_level::{category_draws, FarLevel};
use rs910_far_scene::far_locs::{
    self, container_of, PlaceAssets, PlacedLoc, Placer, CONTAINER_TILES,
};
use rs910_far_scene::far_ring::{SquareId, TileRect};
use rs910_far_scene::far_terrain::TerrainCache;

use crate::far::batch::{merge_chunks, MergedMesh, Part};
use crate::far::lod::{self, ClassicMesh, LodMesh};
use crate::models::mesh::{model_streams, Colour};
use crate::modelunlit::ModelUnlit;
use crate::texture::MaterialStore;

/// What every far job reads, loaded once and shared.
pub(crate) struct FarAssets {
    pub(crate) pack: crate::cache::Pack,
    pub(crate) flo: rs910_config::flo::FloStore,
    pub(crate) place: Arc<PlaceAssets>,
    pub(crate) terrain: TerrainCache,
    /// The RT7 material extras (the texture maps a material load reads).
    pub(crate) extras: Option<rs910_config::nxt::material::Rt7MaterialExtraStore>,
    /// The texture archives' group ids (sorted): the groups a material load
    /// may request.
    pub(crate) texture_indexes: Vec<(&'static str, Vec<u32>)>,
    /// Whether each material checked so far may load quietly
    /// ([`Self::material_quiet`]).
    pub(crate) quiet: std::sync::Mutex<HashMap<i32, bool>>,
    /// The far terrain's texture-array layers decoded so far, by material
    /// (`None`: no layer, drawn untextured).
    pub(crate) layers:
        std::sync::Mutex<HashMap<i32, Option<Arc<crate::terrain::atlas::LayerLevels>>>>,
}

impl FarAssets {
    /// The far scene's own copies of what its builds read: the snapshot's
    /// materials, the flo configs, and the billboard, emitter and RT7
    /// material stores and texture indexes (config and index reads, as the
    /// near RT7 path makes them).
    pub(crate) fn load(
        pack: &crate::cache::Pack,
        materials: &MaterialStore,
        flo: &rs910_config::flo::FloStore,
    ) -> Self {
        let place = PlaceAssets {
            materials: materials.clone(),
            billboards: crate::billboard::BillboardStore::load(pack).unwrap_or_default(),
            emitters: rs910_model::particle::EmitterStore::load(pack).unwrap_or_default(),
            flo: rs910_scene::maploader::FloTables::from_store(flo),
            detail_flags: rs910_scene::rebuild::BuildPrefs::default().model_detail(),
        };
        let texture_indexes = [
            rs910_config::texture::TEXTURES_PNG_ARCHIVE,
            rs910_config::texture::TEXTURES_PNG_MIPPED_ARCHIVE,
            rs910_config::texture::TEXTURES_DXT_ARCHIVE,
            rs910_config::texture::TEXTURES_ETC_ARCHIVE,
        ]
        .into_iter()
        .map(|a| {
            let mut groups = pack
                .read_archive_index(a)
                .map(|i| i.group_id)
                .unwrap_or_default();
            groups.sort_unstable();
            (a, groups)
        })
        .collect();
        Self {
            pack: pack.clone(),
            flo: flo.clone(),
            place: Arc::new(place),
            terrain: TerrainCache::default(),
            extras: rs910_config::nxt::material::Rt7MaterialExtraStore::load(pack).ok(),
            texture_indexes,
            quiet: std::sync::Mutex::new(HashMap::new()),
            layers: std::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Whether the texture groups material `id` may read are resident: a
    /// group an archive's index lists must be resident in every texture
    /// archive a material load may try, so drawing the material never makes
    /// the client request a group. Decided once per material (the loc
    /// workers decide their batches' materials, so the render thread finds
    /// them decided).
    pub(crate) fn material_quiet(&self, id: i32) -> bool {
        if let Some(&q) = self.quiet.lock().expect("far quiet materials").get(&id) {
            return q;
        }
        let q = self.check_material(id);
        self.quiet
            .lock()
            .expect("far quiet materials")
            .insert(id, q);
        q
    }

    fn check_material(&self, id: i32) -> bool {
        let Some(m) = u32::try_from(id).ok().and_then(|i| self.materials().get(i)) else {
            return true;
        };
        let mut textures: Vec<i32> = m
            .diffuse_texture
            .iter()
            .chain(m.aux_texture.iter())
            .map(|&t| t as i32)
            .collect();
        if let Some(extra) = self.extras.as_ref().and_then(|e| e.get(id as u32)) {
            for r in [extra.diffuse, extra.normal, extra.compound]
                .into_iter()
                .flatten()
            {
                textures.push(r.texture);
            }
        }
        textures.into_iter().all(|t| {
            let Ok(group) = u32::try_from(t) else {
                return true;
            };
            self.texture_indexes.iter().all(|(archive, groups)| {
                groups.binary_search(&group).is_err()
                    || self
                        .pack
                        .read_group_resident(archive, group)
                        .is_ok_and(|g| g.is_some())
            })
        })
    }

    /// Material `material`'s far terrain layer, decoded once
    /// (`crate::terrain::atlas::layer_texels`), only when its texture group
    /// is resident, so the far scene never makes the client request a group.
    pub(crate) fn layer(&self, material: i32) -> Option<Arc<crate::terrain::atlas::LayerLevels>> {
        if let Some(l) = self.layers.lock().expect("far layers").get(&material) {
            return l.clone();
        }
        let m = u32::try_from(material)
            .ok()
            .and_then(|id| self.materials().get(id));
        let resident = m.and_then(|m| m.diffuse_texture).is_some_and(|t| {
            self.pack
                .read_group_resident(rs910_config::texture::TEXTURES_PNG_ARCHIVE, t)
                .is_ok_and(|g| g.is_some())
        });
        let texels = m
            .filter(|_| resident)
            .and_then(|m| crate::terrain::atlas::layer_texels(&self.pack, m).ok())
            .map(Arc::new);
        self.layers
            .lock()
            .expect("far layers")
            .insert(material, texels.clone());
        texels
    }

    pub(crate) fn materials(&self) -> &MaterialStore {
        &self.place.materials
    }
}

/// A square's terrain: per level its mesh (square-local), the materials its
/// vertex slots number (slot `i` is `materials[i]`) and their decoded
/// layers.
pub(crate) struct TerrainBuild {
    pub(crate) levels: Vec<Option<crate::terrain::LevelMesh>>,
    pub(crate) materials: Vec<i32>,
    pub(crate) layers: Vec<Option<Arc<crate::terrain::atlas::LayerLevels>>>,
}

/// Build square `sq`'s terrain (`window`: the classic window, whose boundary
/// vertices take the near terrain's normals).
pub(crate) fn terrain(assets: &FarAssets, sq: SquareId, window: TileRect) -> TerrainBuild {
    let squares = assets.terrain.neighbourhood(&assets.pack, sq);
    let mut materials: Vec<i32> = Vec::new();
    let rect = (!window.is_empty()).then_some([window.x0, window.z0, window.x1, window.z1]);
    let levels = if squares.contains_key(&sq) {
        crate::terrain::build_square(
            &squares,
            sq,
            &assets.flo,
            assets.materials(),
            rect,
            &mut |m| {
                if let Some(i) = materials.iter().position(|&x| x == m) {
                    return i as u16;
                }
                materials.push(m);
                (materials.len() - 1) as u16
            },
        )
    } else {
        Vec::new()
    };
    let layers = materials.iter().map(|&m| assets.layer(m)).collect();
    TerrainBuild {
        levels,
        materials,
        layers,
    }
}

/// One loc container's merged mesh and the smallest class it serves (its
/// locs of the categories that class keeps, `far_level::category_draws`).
pub(crate) struct ContainerBuild {
    pub(crate) id: (i32, i32),
    pub(crate) class: u8,
    /// Its merged mesh, in 16-bit chunks.
    pub(crate) mesh: Vec<MergedMesh>,
}

/// How much nearer (fine units) than it is now a container is built for:
/// its locs are those of the class it would have that much nearer, so a
/// camera moving within that reach needs no rebuild.
pub(crate) const REBUILD_REACH: i32 = 8192;

/// The 2D distance, fine units, from `focus` to container `id`'s tiles.
pub(crate) fn container_distance(focus: [i32; 2], id: (i32, i32)) -> i32 {
    let (x0, z0) = (id.0 * CONTAINER_TILES * 512, id.1 * CONTAINER_TILES * 512);
    let (x1, z1) = (x0 + CONTAINER_TILES * 512, z0 + CONTAINER_TILES * 512);
    let axis = |p: i32, lo: i32, hi: i32| (lo - p).max(p - hi).max(0);
    f64::from(axis(focus[0], x0, x1)).hypot(f64::from(axis(focus[1], z0, z1))) as i32
}

/// The class container `id` is built for at `focus` and `level` (`None`:
/// beyond the level even [`REBUILD_REACH`] nearer, nothing to build).
pub(crate) fn build_class(focus: [i32; 2], id: (i32, i32), level: FarLevel) -> Option<u8> {
    rs910_far_scene::far_level::container_class(
        (container_distance(focus, id) - REBUILD_REACH).max(0),
        level,
    )
}

/// What building a square's locs counted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct LocBuildStats {
    pub(crate) place: far_locs::PlaceStats,
    /// Locs drawn from RT7 (with LODs) and from their classic mesh.
    pub(crate) rt7: usize,
    pub(crate) classic: usize,
    /// Locs without drawable geometry.
    pub(crate) empty: usize,
    /// LOD 0 triangles of the square's locs, and those RT7 built for
    /// classic faces (the check: every RT7 loc draws its classic draw faces).
    pub(crate) triangles: usize,
    pub(crate) face_mismatches: usize,
    /// Locs per category (0, 2, 4) and the merged meshes' bytes.
    pub(crate) categories: [usize; 5],
    pub(crate) bytes: usize,
    pub(crate) index_bytes: usize,
}

/// A square's far locs, per container.
pub(crate) struct LocsBuild {
    pub(crate) containers: Vec<ContainerBuild>,
    pub(crate) stats: LocBuildStats,
}

/// One of the classic window's static locs for an extension container: the
/// entity's slot, its RT7 inputs and its scene-local placement.
pub(crate) struct ExtLoc {
    pub(crate) slot: u32,
    pub(crate) loc_id: u32,
    /// The shape the near RT7 path looks the models up by
    /// (`crate::models::rt7::Rt7Cache::streams`).
    pub(crate) shape: i32,
    pub(crate) classic: ClassicMesh,
    /// Scene-local matrix entries (column-major).
    pub(crate) matrix: [f32; 16],
    pub(crate) lights: [f32; 4],
}

/// An extension container's merged mesh and the slots it holds (the
/// others fell back to their classic meshes: the per-loc path draws them).
pub(crate) struct ExtBuild {
    /// Its merged mesh, in 16-bit chunks.
    pub(crate) mesh: Vec<MergedMesh>,
    pub(crate) slots: Vec<u32>,
}

/// A loc worker: the placement state (its own loc configs and model cache)
/// and the decoded RT7 and raw classic models.
pub(crate) struct LocWorker {
    pub(crate) assets: Arc<FarAssets>,
    placer: Placer,
    rt7: HashMap<i32, Option<Arc<Rt7Model>>>,
    raw: HashMap<i32, Option<Arc<ModelUnlit>>>,
}

impl LocWorker {
    /// A worker over `assets` (loads its own copy of the loc configs).
    pub(crate) fn new(assets: Arc<FarAssets>) -> Self {
        let locs = rs910_config::config::LocStore::load(&assets.pack).unwrap_or_else(|err| {
            log::warn!("[modern] far locs: loc configs: {err:#}");
            rs910_config::config::LocStore::default()
        });
        Self {
            placer: Placer::new(assets.pack.clone(), Arc::clone(&assets.place), locs),
            assets,
            rt7: HashMap::new(),
            raw: HashMap::new(),
        }
    }

    fn rt7_model(&mut self, id: i32) -> Option<Arc<Rt7Model>> {
        let pack = &self.assets.pack;
        self.rt7
            .entry(id)
            .or_insert_with(|| {
                let group = u32::try_from(id).ok()?;
                let files = pack.read_group_resident(MODEL_RT7_ARCHIVE, group).ok()??;
                decode_model_rt7(group, files.get(&0)?).ok().map(Arc::new)
            })
            .clone()
    }

    /// Classic model `id` as the loc type loads it (times 4 below version 13),
    /// read quietly.
    fn raw_model(&mut self, id: i32) -> Option<Arc<ModelUnlit>> {
        let pack = &self.assets.pack;
        self.raw
            .entry(id)
            .or_insert_with(|| {
                let group = u32::try_from(id).ok()?;
                let files = pack
                    .read_group_resident(crate::modelunlit::MODEL_ARCHIVE, group)
                    .ok()??;
                let mut m = ModelUnlit::decode(files.get(&0)?).ok()?;
                if m.version < 13 {
                    m.scale_by_power_of_two(2);
                }
                Some(Arc::new(m))
            })
            .clone()
    }

    /// The RT7 geometry with LODs of placed model `classic` of loc `loc_id`
    /// built for `shape` (`Err`: RT7 does not correspond).
    fn rt7_lods(&mut self, loc_id: u32, shape: i32, classic: &ClassicMesh) -> Result<LodMesh, ()> {
        let (ids, y_scale) = {
            let Some(loc) = self.placer.locs.get(loc_id) else {
                return Err(());
            };
            let Some((_, ids)) = loc
                .shape_models
                .iter()
                .find(|(s, _)| i32::from(*s) == shape)
            else {
                return Err(());
            };
            (ids.clone(), loc.resizey as f32 / 128.0)
        };
        if ids.is_empty() {
            return Err(());
        }
        let mut parts = Vec::with_capacity(ids.len());
        let mut raws = Vec::with_capacity(ids.len());
        for &id in &ids {
            let (Some(rt7), Some(raw)) = (self.rt7_model(id), self.raw_model(id)) else {
                return Err(());
            };
            parts.push(rt7);
            raws.push(raw);
        }
        let merged;
        let raw: &ModelUnlit = if raws.len() == 1 {
            &raws[0]
        } else {
            let refs: Vec<&ModelUnlit> = raws.iter().map(|r| r.as_ref()).collect();
            merged = ModelUnlit::merge(&refs);
            &merged
        };
        lod::build(classic, raw, &parts, y_scale).map_err(|_| ())
    }

    /// Square `sq`'s far locs outside `window`, each container with the
    /// categories its class at `focus` and `level` keeps ([`build_class`];
    /// see the module docs).
    pub(crate) fn square(
        &mut self,
        sq: SquareId,
        window: TileRect,
        focus: [i32; 2],
        level: FarLevel,
    ) -> LocsBuild {
        let keep = |category: u8, (x, z): (i32, i32)| {
            build_class(focus, container_of(x, z), level)
                .is_some_and(|c| category_draws(category, c))
        };
        let placed = far_locs::place_square(&mut self.placer, sq, window, &keep);
        let mut stats = LocBuildStats {
            place: placed.stats,
            ..LocBuildStats::default()
        };
        let materials = Arc::clone(&self.assets);
        let mut by_container: BTreeMap<(i32, i32), Vec<(LodMesh, &PlacedLoc)>> = BTreeMap::new();
        for loc in &placed.locs {
            let Some(classic) = ClassicMesh::of(&loc.model, materials.materials()) else {
                stats.empty += 1;
                continue;
            };
            let mesh = match self.rt7_lods(loc.loc_id, loc.model_shape, &classic) {
                Ok(mesh) => {
                    stats.rt7 += 1;
                    stats.face_mismatches += usize::from(mesh.triangles(0) != classic.rt7_faces());
                    mesh
                }
                Err(()) => {
                    let Some(s) = model_streams(&loc.model, materials.materials(), Colour::Classic)
                    else {
                        stats.empty += 1;
                        continue;
                    };
                    stats.classic += 1;
                    LodMesh::single(s.vertices, s.colours, &s.indices, &s.batches)
                }
            };
            stats.triangles += mesh.triangles(0);
            stats.categories[usize::from(loc.category.min(4))] += 1;
            let (tx, tz) = loc.tile();
            by_container
                .entry(container_of(tx, tz))
                .or_default()
                .push((mesh, loc));
        }
        let containers: Vec<ContainerBuild> = by_container
            .into_iter()
            .map(|(id, locs)| {
                let origin = [
                    id.0 * CONTAINER_TILES * 512,
                    0,
                    id.1 * CONTAINER_TILES * 512,
                ];
                let parts = locs.iter().enumerate().map(|(i, (mesh, loc))| {
                    let pos = [
                        (loc.pos[0] - origin[0]) as f32,
                        loc.pos[1] as f32,
                        (loc.pos[2] - origin[2]) as f32,
                    ];
                    Part {
                        slot: i as u32,
                        mesh,
                        matrix: placement(loc.srt, pos),
                        lights: [0.0; 4],
                        transparent: loc.transparent,
                        category: loc.category,
                    }
                });
                ContainerBuild {
                    id,
                    class: build_class(focus, id, level).unwrap_or(4),
                    mesh: merge_chunks(origin, parts),
                }
            })
            .collect();
        stats.bytes = containers
            .iter()
            .flat_map(|c: &ContainerBuild| &c.mesh)
            .map(|m| m.bytes() as usize)
            .sum();
        stats.index_bytes = containers
            .iter()
            .flat_map(|c| &c.mesh)
            .map(|m| m.indices.len() * 2)
            .sum();
        for m in containers
            .iter()
            .flat_map(|c| &c.mesh)
            .flat_map(|m| &m.batches)
        {
            self.assets.material_quiet(m.material);
        }
        LocsBuild { containers, stats }
    }

    /// One extension container at absolute container `id` from `locs`
    /// (`base`: the scene's base tile; see the module docs).
    pub(crate) fn extension(
        &mut self,
        id: (i32, i32),
        base: [i32; 2],
        locs: Vec<ExtLoc>,
    ) -> ExtBuild {
        let origin = [
            id.0 * CONTAINER_TILES * 512,
            0,
            id.1 * CONTAINER_TILES * 512,
        ];
        // Scene-local -> container-local.
        let shift = [
            (base[0] * 512 - origin[0]) as f32,
            (base[1] * 512 - origin[2]) as f32,
        ];
        let mut meshes = Vec::with_capacity(locs.len());
        for loc in &locs {
            if let Ok(mesh) = self.rt7_lods(loc.loc_id, loc.shape, &loc.classic) {
                meshes.push((mesh, loc));
            }
        }
        let slots = meshes.iter().map(|(_, l)| l.slot).collect();
        let parts = meshes.iter().map(|(mesh, loc)| {
            let mut matrix = loc.matrix;
            matrix[12] += shift[0];
            matrix[14] += shift[1];
            Part {
                slot: loc.slot,
                mesh,
                matrix,
                lights: loc.lights,
                transparent: false,
                // The window's locs are all drawn (the extension keeps the
                // near path's frame).
                category: 4,
            }
        });
        let mesh = merge_chunks(origin, parts);
        for m in mesh.iter().flat_map(|m| &m.batches) {
            self.assets.material_quiet(m.material);
        }
        ExtBuild { mesh, slots }
    }
}

/// A placed loc's matrix: its scale and rotation with its position, or the
/// position alone (`rs910_scene::scene::entity_srt`'s rule).
fn placement(srt: Option<rs910_scene::map::LocSrt>, pos: [f32; 3]) -> [f32; 16] {
    match srt.filter(|s| s.rot != [0.0, 0.0, 0.0, 1.0] || s.scale != [1.0; 3]) {
        Some(s) => crate::actor_matrix::Matrix::srt(s.rot, s.scale, pos).entries(),
        None => [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, pos[0], pos[1], pos[2], 1.0,
        ],
    }
}
