//! Far locs (`docs/renderer/modern-renderer.md`):
//! the static locs of one map square outside the classic
//! window, placed by the classic placement itself on a private scene, and
//! the loc containers that batch them.
//!
//! **Placement.** [`place_square`] runs the classic map build's loc slice
//! (`rs910_scene::locs`) over a private 3 x 3 square scene: the square's and
//! its neighbours' LAND heights (the missing squares cleared as the classic
//! build clears them), then the square's LOC file.
//! Every placed loc carries the lit model the loc type's model builder
//! gave it (mirror, rotation, scale, offsets, `hillchange` against the
//! LAND heights, post offsets) and its entity position, exactly what the
//! classic window would hold for it. Dynamic locs (animated, multilocs) get
//! their unanimated base model (design §6.2: frame 0, static; a multiloc
//! without models of its own is skipped, a gap against the modern client,
//! which resolves it with the local player's vars).
//!
//! **Inertness.** The map groups and every model group are read with
//! `Pack::read_group_resident` first (a loc whose models are not resident
//! is left out, so the classic model loader below never meets an absent group
//! and never queues a JS5 request); the model source is private
//! (`ModelSource::new` makes its own 500-entry model cache, never the classic
//! one); the loc configs are the far scene's own copy. Nothing reads or
//! writes game or scene state.
//!
//! **Exclusion.** A loc the classic window admits (the classic loc filter,
//! in window coordinates) is never placed here: the window draws it.
//!
//! **Containers.** [`CONTAINER_TILES`]: the modern client groups locs into
//! 16 x 16-tile containers (`x >> 4`, `z >> 4`, always; 4 x 4 per map
//! square), each drawn as merged batches.
//!
//! **Categories.** Each loc has a level-of-detail category ([`category`])
//! that decides from which container class on it is left
//! out (`far_level::category_draws`).

use std::collections::HashMap;
use std::sync::Arc;

use rs910_config::billboard::BillboardStore;
use rs910_config::config::{Loc, LocStore};
use rs910_config::texture::MaterialStore;
use rs910_js5::cache::{Pack, LAND_FILE, LOC_FILE};
use rs910_model::gpumodel::GpuModel;
use rs910_model::particle::EmitterStore;
use rs910_scene::locs::{LocPlacer, LocStats, PlacePrefs};
use rs910_scene::loctype::{self, shape, ModelSource, SharedModelCache};
use rs910_scene::map::LocSrt;
use rs910_scene::maploader::{FloTables, MapLoader, SceneFloors};
use rs910_scene::scene::Scene;
use rs910_scene::tileflags::SceneLevelTileFlags;

use crate::far_ring::{in_world, SquareId, TileRect, SQUARE_TILES};

/// Tiles per loc container side (see the module docs).
pub const CONTAINER_TILES: i32 = 16;

/// A loc's level-of-detail category: 0 (drawn only in
/// class-0 containers), 2 (classes 0-2) or 4 (every class). Opcode 196
/// below 5 names it (1 is 0, 3 is 2, the rest as they are); otherwise
/// (automatic) opcode 197 = 1 gives 0, walls, diagonal walls and shape 23
/// give 2, ground and wall decorations and shape 24 give 0, roofs and roof
/// edges give 2, and the other shapes (centrepieces) give 2 unless their
/// footprint is 1 x 1. For a centrepiece without opcode 197 = 2, the modern
/// client also reads its tile flags under the footprint (every tile with
/// bit 4 gives 0); those flags are not traced, so they are taken as unset
/// (a stand-in).
#[must_use]
pub fn category(loc: &Loc, shape: i32) -> u8 {
    let [by_type, hint] = loc.nxt_lod;
    if by_type < 5 {
        return match by_type {
            1 => 0,
            3 => 2,
            v => v,
        };
    }
    if hint == 1 {
        return 0;
    }
    match shape {
        0..=3 | 9 | 23 | 12..=21 => 2,
        4..=8 | 22 | 24 => 0,
        _ => {
            if loc.width == 1 && loc.length == 1 {
                0
            } else {
                2
            }
        }
    }
}

/// The container holding absolute tile `(x, z)`.
#[must_use]
pub fn container_of(x: i32, z: i32) -> (i32, i32) {
    (x.div_euclid(CONTAINER_TILES), z.div_euclid(CONTAINER_TILES))
}

/// One map group's files by id.
type MapFiles = std::collections::BTreeMap<u32, Vec<u8>>;

/// The private scene of one square's placement: the square and its eight
/// neighbours.
const SCENE_TILES: usize = 3 * SQUARE_TILES as usize;

/// What the placement reads besides the pack and the loc configs, loaded
/// once and shared by the placement workers.
pub struct PlaceAssets {
    pub materials: MaterialStore,
    pub billboards: BillboardStore,
    pub emitters: EmitterStore,
    pub flo: FloTables,
    /// `modelDetailFlags` (`BuildPrefs::model_detail` at the default
    /// preferences).
    pub detail_flags: i32,
}

/// One placement worker's own state: the loc configs (not `Sync`, so each
/// worker holds its copy) and its private model cache.
pub struct Placer {
    pub pack: Pack,
    pub assets: Arc<PlaceAssets>,
    pub locs: LocStore,
    pub model_cache: SharedModelCache,
    /// Model ids found resident (or not) so far.
    resident: HashMap<i32, bool>,
}

impl Placer {
    #[must_use]
    pub fn new(pack: Pack, assets: Arc<PlaceAssets>, locs: LocStore) -> Self {
        Self {
            pack,
            assets,
            locs,
            model_cache: SharedModelCache::default(),
            resident: HashMap::new(),
        }
    }

    /// Whether model `id` is resident (read quietly; see the module docs).
    fn model_resident(&mut self, id: i32) -> bool {
        let pack = &self.pack;
        *self.resident.entry(id).or_insert_with(|| {
            u32::try_from(id).is_ok_and(|group| {
                pack.read_group_resident(rs910_model::modelunlit::MODEL_ARCHIVE, group)
                    .is_ok_and(|g| g.is_some())
            })
        })
    }
}

/// Which scene-graph layer a placed loc sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LocKind {
    Scenery,
    Wall,
    WallDecor,
    GroundDecor,
}

/// One placed far loc.
#[derive(Clone, Debug)]
pub struct PlacedLoc {
    pub kind: LocKind,
    pub loc_id: u32,
    /// The shape its model was built for (the model builder's
    /// argument: a diagonal centrepiece as the straight one, every wall
    /// decoration as the straight one without offset).
    pub model_shape: i32,
    pub angle: i32,
    /// The stored plane.
    pub level: i32,
    /// Absolute fine position of the entity (y down), the wall
    /// decoration's draw offset included.
    pub pos: [i32; 3],
    /// The absolute tile of the entity's own position (without the wall
    /// decoration's offset): where the classic scene graph holds it.
    pub anchor: (i32, i32),
    pub srt: Option<LocSrt>,
    /// The classic transparent list (the model's transparency).
    pub transparent: bool,
    /// Placed as a dynamic placeholder (its model is the unanimated base).
    pub dynamic: bool,
    /// Its level-of-detail category ([`category`]).
    pub category: u8,
    pub model: GpuModel,
}

impl PlacedLoc {
    /// The absolute tile of the entity's position (its container's).
    #[must_use]
    pub fn tile(&self) -> (i32, i32) {
        self.anchor
    }
}

/// What [`place_square`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PlaceStats {
    pub spawns: usize,
    /// Left to the classic window (it admits them).
    pub in_window: usize,
    /// Left out by the caller's `keep` (their category does not draw
    /// where they stand).
    pub culled: usize,
    /// Left out: a model group not resident.
    pub not_resident: usize,
    pub placed: usize,
    /// Dynamic placeholders given their base model.
    pub dynamic: usize,
    /// Placed without a model (none for the shape, a multiloc).
    pub no_model: usize,
}

/// The placed far locs of one square.
#[derive(Default)]
pub struct SquareLocs {
    pub locs: Vec<PlacedLoc>,
    pub stats: PlaceStats,
}

/// Whether the classic window `window` admits a loc at absolute tile `(x, z)`
/// (the footprint `w x l` by angle must reach into the window, and only
/// centrepieces may sit on its border tiles).
#[must_use]
pub fn window_admits(window: &TileRect, loc: &Loc, x: i32, z: i32, angle: i32, shape: i32) -> bool {
    if window.is_empty() {
        return false;
    }
    let (w, l) = if angle & 1 == 0 {
        (i32::from(loc.width), i32::from(loc.length))
    } else {
        (i32::from(loc.length), i32::from(loc.width))
    };
    let (lx, lz) = (x - window.x0, z - window.z0);
    let (sx, sz) = (window.x1 - window.x0, window.z1 - window.z0);
    if lx >= sx || lz >= sz || lx + w <= 0 || lz + l <= 0 {
        return false;
    }
    shape == shape::CENTREPIECE_STRAIGHT
        || shape == shape::CENTREPIECE_DIAGONAL
        || !(lx <= 0 || lz <= 0 || lx >= sx - 1 || lz >= sz - 1)
}

/// The shape the model builder builds a placed loc of `shape` with
/// (see [`PlacedLoc::model_shape`]).
#[must_use]
pub fn model_shape(shape: i32) -> i32 {
    if shape == shape::CENTREPIECE_DIAGONAL {
        shape::CENTREPIECE_STRAIGHT
    } else if shape::is_wall_decor(shape) {
        shape::WALLDECOR_STRAIGHT_NOOFFSET
    } else {
        shape
    }
}

/// Place square `sq`'s static locs outside `window` that `keep` keeps (by
/// category and spawn tile: a container draws only the categories its
/// class keeps, so the others need not be built; see the module docs).
pub fn place_square(
    placer: &mut Placer,
    sq: SquareId,
    window: TileRect,
    keep: &dyn Fn(u8, (i32, i32)) -> bool,
) -> SquareLocs {
    let mut out = SquareLocs::default();
    if !in_world(sq) {
        return out;
    }
    let base = [(sq.0 - 1) * SQUARE_TILES, (sq.1 - 1) * SQUARE_TILES];
    // The 3 x 3 squares' groups, quietly.
    let mut groups: Vec<(SquareId, Option<MapFiles>)> = Vec::new();
    for dz in -1..=1 {
        for dx in -1..=1 {
            let n = (sq.0 + dx, sq.1 + dz);
            let files = in_world(n)
                .then(|| {
                    let group = rs910_config::nxt::map_group(n.0 as u32, n.1 as u32);
                    placer
                        .pack
                        .read_group_resident(rs910_config::nxt::MAP_ARCHIVE, group)
                        .ok()
                        .flatten()
                })
                .flatten();
            groups.push((n, files));
        }
    }
    let centre = &groups[4].1;
    let Some(loc_bytes) = centre.as_ref().and_then(|f| f.get(&LOC_FILE)) else {
        return out;
    };
    if centre.as_ref().and_then(|f| f.get(&LAND_FILE)).is_none() {
        return out;
    }
    let spawns =
        match rs910_scene::map::decode_locs(loc_bytes, sq.0 * SQUARE_TILES, sq.1 * SQUARE_TILES) {
            Ok(s) => s,
            Err(err) => {
                log::warn!("[far] square {sq:?}: locs: {err}");
                return out;
            }
        };
    out.stats.spawns = spawns.len();
    // Leave out what the window admits and what cannot load quietly.
    let mut kept = Vec::with_capacity(spawns.len());
    for s in spawns {
        let Some(loc) = placer.locs.get(s.id) else {
            continue;
        };
        let (angle, sh) = (i32::from(s.angle), i32::from(s.shape));
        if window_admits(&window, loc, s.x, s.z, angle, sh) {
            out.stats.in_window += 1;
            continue;
        }
        if !keep(category(loc, sh), (s.x, s.z)) {
            out.stats.culled += 1;
            continue;
        }
        let ids: Vec<i32> = loc
            .shape_models
            .iter()
            .find(|(m, _)| i32::from(*m) == model_shape(sh))
            .map(|(_, ids)| ids.clone())
            .unwrap_or_default();
        if !ids.into_iter().all(|id| placer.model_resident(id)) {
            out.stats.not_resident += 1;
            continue;
        }
        kept.push(s);
    }
    // The LAND heights of the 3 x 3 squares.
    let assets = Arc::clone(&placer.assets);
    let mut loader = MapLoader::new(
        &assets.flo,
        &assets.materials,
        4,
        SCENE_TILES,
        SCENE_TILES,
        false,
    );
    loader.enable_blending();
    let mut flags = SceneLevelTileFlags::new(4, SCENE_TILES, SCENE_TILES);
    let mut missing = Vec::new();
    for (n, files) in &groups {
        let (tx, tz) = (n.0 * SQUARE_TILES - base[0], n.1 * SQUARE_TILES - base[1]);
        let land = files.as_ref().and_then(|f| f.get(&LAND_FILE));
        let read = land.is_some_and(|land| {
            let mut packet = rs910_config::landscape_packet::Packet::new(land);
            loader
                .read_normal_landscape(&mut packet, &mut flags, tx, tz, base[0], base[1])
                .is_ok()
        });
        if !read {
            missing.push((tx, tz));
        }
    }
    for (tx, tz) in missing {
        loader.clear_landscape(tx, tz, SQUARE_TILES, SQUARE_TILES);
    }
    let mut floors = SceneFloors::new(4, SCENE_TILES, SCENE_TILES, false);
    loader.build_floors(&mut floors, None);
    let source = ModelSource::new(
        &placer.pack,
        &assets.materials,
        &assets.billboards,
        &assets.emitters,
        assets.detail_flags,
    )
    .with_model_cache(placer.model_cache.clone());
    let mut scene = Scene::new(9, 4, SCENE_TILES, SCENE_TILES);
    let sun = rs910_scene::env::Environment::default().sun_lighting([0.0, -1.0, 0.0], 3, 0.0);
    {
        let mut locs_placer = LocPlacer {
            scene: &mut scene,
            floors: &mut floors.normal_builders,
            level_occludemap: &mut loader.level_occludemap,
            locs: &placer.locs,
            models: &source,
            // No hard-shadow stamps (the far scene draws no classic floor).
            prefs: PlacePrefs {
                scenery_shadows: 0,
                ..PlacePrefs::default()
            },
            is_blending: true,
            underwater: false,
            levels: 4,
            min_level: 99,
            stats: LocStats::default(),
            loc_tint: (0, 0, 0, 0),
            sun,
            sounds: Vec::new(),
        };
        if let Err(err) = locs_placer.read_normal_locs(&kept, base[0], base[1]) {
            log::warn!("[far] square {sq:?}: placement: {err:#}");
        }
    }
    // The placed entities, absolute.
    let heights: Vec<Option<&rs910_model::floor::FloorHeights>> = floors
        .normal_builders
        .iter()
        .map(|b| b.as_ref().map(|b| &b.heights))
        .collect();
    let abs = |x: i32, y: i32, z: i32| [x + base[0] * 512, y, z + base[1] * 512];
    let push = |out: &mut SquareLocs,
                kind: LocKind,
                loc_id: u32,
                sh: i32,
                angle: i32,
                level: i32,
                occlude_level: i32,
                pos: [i32; 3],
                model_pos: [i32; 3],
                srt: Option<LocSrt>,
                model: Option<&GpuModel>,
                dynamic: bool| {
        let model = match model {
            Some(m) => Some(m.clone()),
            None if dynamic => placer.locs.get(loc_id).and_then(|loc| {
                let floor = heights.get(occlude_level as usize).copied().flatten();
                let above = if occlude_level < 3 {
                    heights.get(occlude_level as usize + 1).copied().flatten()
                } else {
                    None
                };
                let flags = 2048 | if loc.antimacro { 0x80000 } else { 0 };
                let msh = model_shape(sh);
                let mang = if sh == shape::CENTREPIECE_DIAGONAL {
                    angle + 4
                } else {
                    angle
                };
                loctype::get_dynamic_model(
                    &source,
                    loc,
                    loctype::ModelRequest {
                        flags,
                        shape: msh,
                        rotation: mang,
                    },
                    loctype::LocGround { floor, above },
                    [model_pos[0], model_pos[1], model_pos[2]],
                )
                .ok()
                .flatten()
            }),
            None => None,
        };
        let Some(model) = model else {
            out.stats.no_model += 1;
            return;
        };
        out.stats.placed += 1;
        out.stats.dynamic += usize::from(dynamic);
        let category = placer.locs.get(loc_id).map_or(0, |loc| category(loc, sh));
        out.locs.push(PlacedLoc {
            kind,
            loc_id,
            model_shape: model_shape(sh),
            angle,
            level,
            pos,
            anchor: (
                model_pos[0].div_euclid(512) + base[0],
                model_pos[2].div_euclid(512) + base[1],
            ),
            srt,
            transparent: model.has_transparency,
            dynamic,
            category,
            model,
        });
    };
    for e in &scene.scenery {
        push(
            &mut out,
            LocKind::Scenery,
            e.loc_id,
            e.shape,
            e.angle,
            e.level,
            e.occlude_level,
            abs(e.x, e.y, e.z),
            [e.x, e.model_y, e.z],
            e.srt,
            e.model.as_ref(),
            e.dynamic,
        );
    }
    for e in &scene.walls {
        push(
            &mut out,
            LocKind::Wall,
            e.loc_id,
            e.shape,
            e.angle,
            e.level,
            e.occlude_level,
            abs(e.x, e.y, e.z),
            [e.x, e.y, e.z],
            e.srt,
            e.model.as_ref(),
            e.dynamic,
        );
    }
    for e in &scene.wall_decors {
        push(
            &mut out,
            LocKind::WallDecor,
            e.loc_id,
            e.shape,
            e.angle,
            e.level,
            e.occlude_level,
            abs(e.x + e.offset_x, e.y, e.z + e.offset_z),
            [e.x, e.y, e.z],
            e.srt,
            e.model.as_ref(),
            e.dynamic,
        );
    }
    for e in &scene.ground_decors {
        push(
            &mut out,
            LocKind::GroundDecor,
            e.loc_id,
            shape::GROUND_DECOR,
            e.angle,
            e.level,
            e.occlude_level,
            abs(e.x, e.y, e.z),
            [e.x, e.y, e.z],
            e.srt,
            e.model.as_ref(),
            e.dynamic,
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(width: u8, length: u8) -> Loc {
        let mut l = rs910_config::config::decode_loc(1, &[0]).expect("an empty loc");
        l.width = width;
        l.length = length;
        l
    }

    /// The category rule: the type's own category, the automatic one by
    /// shape and footprint.
    #[test]
    fn categories_follow_the_type_then_the_shape() {
        let mut l = loc(1, 1);
        assert_eq!(category(&l, shape::WALL_STRAIGHT), 2);
        assert_eq!(category(&l, shape::GROUND_DECOR), 0);
        assert_eq!(category(&l, shape::ROOF_STRAIGHT), 2);
        assert_eq!(category(&l, shape::CENTREPIECE_STRAIGHT), 0);
        assert_eq!(category(&loc(2, 1), shape::CENTREPIECE_STRAIGHT), 2);
        l.nxt_lod = [5, 1];
        assert_eq!(category(&l, shape::WALL_STRAIGHT), 0);
        l.nxt_lod = [3, 0];
        assert_eq!(category(&l, shape::GROUND_DECOR), 2);
        l.nxt_lod = [4, 0];
        assert_eq!(category(&l, shape::GROUND_DECOR), 4);
    }

    /// The window's admission is the classic one: a footprint reaching into the
    /// window counts, border tiles admit only centrepieces, a rotated
    /// footprint swaps its sides.
    #[test]
    fn window_admission_is_readnormallocs_filter() {
        let w = TileRect::at([3160, 3160], [104, 104]);
        let one = loc(1, 1);
        let wide = loc(3, 1);
        // Inside.
        assert!(window_admits(&w, &one, 3200, 3200, 0, shape::WALL_STRAIGHT));
        // On the west border tile: only centrepieces.
        assert!(!window_admits(
            &w,
            &one,
            3160,
            3200,
            0,
            shape::WALL_STRAIGHT
        ));
        assert!(window_admits(
            &w,
            &one,
            3160,
            3200,
            0,
            shape::CENTREPIECE_STRAIGHT
        ));
        // West of the window, its footprint reaching in (x + w > 0).
        assert!(window_admits(
            &w,
            &wide,
            3158,
            3200,
            0,
            shape::CENTREPIECE_STRAIGHT
        ));
        assert!(!window_admits(
            &w,
            &wide,
            3157,
            3200,
            0,
            shape::CENTREPIECE_STRAIGHT
        ));
        // Rotated a quarter, the footprint is 1 x 3: x no longer reaches.
        assert!(!window_admits(
            &w,
            &wide,
            3158,
            3200,
            1,
            shape::CENTREPIECE_STRAIGHT
        ));
        // Beyond the east edge.
        assert!(!window_admits(
            &w,
            &one,
            3264,
            3200,
            0,
            shape::CENTREPIECE_STRAIGHT
        ));
        assert!(!window_admits(&TileRect::default(), &one, 0, 0, 0, 10));
    }
}
