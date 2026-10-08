//! The scene rebuild: the floor slice of a normal map rebuild, offline over the js5 pack.
//!
//! What runs:
//! - window + base tile (`sceneBaseTile = (regionX - mapSize/16) * 8`), map square group id
//!   (`mx | mz << 7`), standard build area 104;
//! - landscape read per square (missing squares are cleared);
//! - underwater pass when `waterDetail == 2` and any square carries the underwater land
//!   file (file 4);
//! - floor build, wall shade stamps from the loc placement (walls only — see
//!   [`crate::maploader::MapLoader::stamp_wall_shadows`]), ground build, underwater ground
//!   build, then blending is disabled.
//!
//! Remaining presentation work includes the partial environment update (the default sun is
//! still used for floor baking), entities and NPCs.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Context;

use crate::cache::{self, Pack};
use crate::config::LocStore;
use crate::locs::{LocPlacer, LocStats, PlacePrefs};
use crate::loctype::ModelSource;
use crate::map::{self, LocSpawn};
use crate::maploader::{FloTables, MapLoader, Packet, SceneFloors};
use crate::protocol910::terrain::RegionCopy;
use crate::scene::Scene;
use crate::texture::MaterialStore;
use crate::tileflags::SceneLevelTileFlags;

/// Side length of the standard build area, in tiles.
pub const MAP_SIZE_STANDARD: usize = 104;
/// Map file id of the underwater land data.
pub const UNDERWATER_LAND_FILE: u32 = 4;
/// Map file id of the underwater loc data.
pub const UNDERWATER_LOC_FILE: u32 = 1;

/// The graphics preferences the floor build reads
/// (defaults come from the client options).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BuildPrefs {
    /// `sceneryShadows` (default 2; forced 0 when textures are off).
    pub scenery_shadows: i32,
    /// `waterDetail` (default 1; `2` enables the underwater floor).
    pub water_detail: i32,
    /// `lightingDetail` (default 1).
    pub lighting_detail: i32,
    /// `groundBlending` (default 1).
    pub ground_blending: i32,
    /// `textures` (default 1).
    pub textures: i32,
    /// `brightness` (default 3) — feeds the ambient sun term.
    pub brightness: i32,
    pub ground_decoration: i32,
    pub anim_detail: i32,
}

impl Default for BuildPrefs {
    fn default() -> Self {
        Self {
            scenery_shadows: 2,
            water_detail: 1,
            lighting_detail: 1,
            ground_blending: 1,
            textures: 1,
            brightness: 3,
            ground_decoration: 1,
            anim_detail: 1,
        }
    }
}

impl BuildPrefs {
    pub fn from_options(options: &crate::client_options::ClientOptions) -> Self {
        let value = |field| options.get(field).expect("preference field");
        Self {
            scenery_shadows: value("sceneryShadows"),
            water_detail: value("waterDetail"),
            lighting_detail: value("lightingDetail"),
            ground_blending: value("groundBlending"),
            textures: value("textures"),
            brightness: value("brightness"),
            ground_decoration: value("groundDecoration"),
            anim_detail: value("animDetail"),
        }
    }

    /// Model detail flags derived from the lighting and texture preferences.
    pub fn model_detail(&self) -> i32 {
        (if self.lighting_detail == 1 {
            crate::gpumodel::MODEL_DETAIL_FLAGS
        } else {
            0
        }) | if self.textures == 0 {
            crate::gpumodel::MODEL_DETAIL_NO_TEXTURES
        } else {
            0
        }
    }

    fn placement(&self) -> PlacePrefs {
        PlacePrefs {
            scenery_shadows: self.scenery_shadows,
            textures: self.textures,
            ground_decoration: self.ground_decoration,
            anim_detail: self.anim_detail,
        }
    }

    fn apply(&self, loader: &mut MapLoader<'_>) {
        loader.enable_blending();
        loader.scenery_shadows = if self.textures == 0 {
            0
        } else {
            self.scenery_shadows
        };
        loader.is_water_detail = self.water_detail == 2;
        loader.is_lighting_detail = self.lighting_detail == 1;
        loader.is_ground_blending = self.ground_blending == 1;
        loader.is_texturing = self.textures == 1;
    }
}

/// One map square of the rebuild window.
#[derive(Clone, Debug)]
pub struct SquareFiles {
    /// `mx << 8 | mz` (`rebuildMapSquares` packing).
    pub packed: u32,
    /// Js5 group id (`mx | mz << 7`).
    pub group: u32,
    /// LAND (file 3) bytes.
    pub land: Option<Vec<u8>>,
    /// LOC (file 0) bytes, if present.
    pub loc: Option<Vec<u8>>,
    /// UNDERWATER_LAND (file 4) bytes, if present.
    pub underwater_land: Option<Vec<u8>>,
    /// UNDERWATER_LOC (file 1) bytes, if present.
    pub underwater_loc: Option<Vec<u8>>,
}

/// Result of [`rebuild_normal`].
#[derive(Debug)]
pub struct Rebuild {
    /// Scene floors (normal + underwater), finished.
    pub scene: SceneFloors,
    /// Tile flags read from the LAND streams.
    pub flags: SceneLevelTileFlags,
    /// `levelOccludemap` from the normal loader (`(size+1)^2` per level).
    #[allow(
        dead_code,
        reason = "the occlusion map of the build; nothing reads it yet"
    )]
    pub occludemap: Vec<Vec<u8>>,
    /// Scene base tile X (`sceneBaseTile.x`).
    pub base_x: i32,
    /// Scene base tile Z.
    pub base_z: i32,
    /// Build area size (tiles per side).
    pub map_size: usize,
    /// Squares that were loaded, in `rebuildMapSquares` order.
    pub squares: Vec<u32>,
    /// Static locs of the window, absolute coords (all levels), only when a
    /// [`LocStore`] was supplied.
    pub locs: Vec<LocSpawn>,
    /// The placed scene graph (`readNormalLocs` + `build` + `reset`), only
    /// when a [`LocStore`] was supplied.
    pub scene_graph: Option<Scene>,
    /// The underwater scene's placed locs, drawn in the underwater pass.
    pub underwater_models: Vec<UnderwaterModel>,
    /// Placement counters.
    pub loc_stats: LocStats,
    /// The placement's positional-sound registrations and the loc types they read.
    pub loc_sounds: crate::loc_sound::LocSoundScene,
    /// Model ids the placement decoded (for the raw-input dump).
    pub model_ids: Vec<u32>,
    pub model_cache: crate::loctype::SharedModelCache,
    /// The environment map, static lights, camera height offsets and
    /// force-ground flags read from the LAND trailers
    /// (the environment trailer read), with the centre sun direction adopted.
    pub env: crate::env::EnvState,
    /// Static lighting: per level, the baked light meshes.
    pub lights: Vec<Vec<crate::floorlight::BakedLight>>,
    /// Wall-clock time of the whole build.
    pub elapsed_ms: u128,
}

/// One placed underwater loc model with its water fog: the tile's `WaterFogData` and the
/// normal level-0 fine height at the entity translation give the height-fog plane
/// `(0, 1, 0, -height) * 3 / scale` and colour.
#[derive(Clone, Debug)]
pub struct UnderwaterModel {
    pub model: crate::gpumodel::GpuModel,
    /// Scene-local fine translation.
    pub position: [i32; 3],
    pub transparent: bool,
    pub fog_plane: [f32; 4],
    pub fog_colour: [f32; 4],
    /// The entity's underwater draw-list inputs (opaque / transparent / pending), `fields[0]`
    /// being its index in the underwater model list.
    pub entity: crate::draw_entity::DrawEntity,
    /// The water fog handed to the renderer: the normal level-0 fine height and the tile's
    /// `WaterFogData` colour/scale.
    pub water_height: i32,
    pub water_colour: i32,
    pub water_scale: i32,
}

/// Every model of the underwater scene with its per-object water fog, in
/// underwater draw-list order (`draw::entities`). Primary-layer entities take their fog
/// tile from their minimum scene tile, others from the translation's tile, clamped into the
/// scene.
fn underwater_models_of(
    graph: &mut Scene,
    fog: &crate::maploader::WaterFogGrid,
    normal0: &crate::floor::FloorHeights,
) -> Vec<UnderwaterModel> {
    use crate::scene::EntityRef;
    let max = [graph.max_x as i32 - 1, graph.max_z as i32 - 1];
    let own = |p: [i32; 3]| [p[0] >> 9, p[2] >> 9];
    let mut out = Vec::new();
    for mut entity in crate::draw_entity::entities(graph) {
        let (model, pos, tile) = match entity.source {
            EntityRef::Scenery(i) => {
                let e = &graph.scenery[i];
                (&e.model, [e.x, e.y, e.z], [e.min_tx, e.min_tz])
            }
            EntityRef::Wall(i) => {
                let e = &graph.walls[i];
                let p = [e.x, e.y, e.z];
                (&e.model, p, own(p))
            }
            EntityRef::WallDecor(i) => {
                let e = &graph.wall_decors[i];
                let p = [e.x + e.offset_x, e.y, e.z + e.offset_z];
                (&e.model, p, own(p))
            }
            EntityRef::GroundDecor(i) => {
                let e = &graph.ground_decors[i];
                let p = [e.x, e.y, e.z];
                (&e.model, p, own(p))
            }
            EntityRef::Temporary(_) => continue,
        };
        let Some(model) = model.as_ref() else {
            continue;
        };
        let [x, z] = [tile[0].clamp(0, max[0]), tile[1].clamp(0, max[1])];
        let data = fog.get(x as usize, z as usize);
        let height = normal0.get_fine_height(pos[0], pos[2]);
        let k = 3.0 / data.scale as f32;
        entity.id = out.len() as i32;
        out.push(UnderwaterModel {
            transparent: model.has_transparency,
            model: model.clone(),
            position: pos,
            fog_plane: [0.0, k, 0.0, -(height as f32) * k],
            fog_colour: [
                ((data.colour >> 16) & 0xFF) as f32 / 255.0,
                ((data.colour >> 8) & 0xFF) as f32 / 255.0,
                (data.colour & 0xFF) as f32 / 255.0,
                0.0,
            ],
            water_height: height,
            water_colour: data.colour,
            water_scale: data.scale,
            entity,
        });
    }
    out
}

/// Squares of the standard window around zone `(region_x, region_z)`
/// packed `mx << 8 | mz`, x outer.
#[must_use]
pub fn window_squares(region_x: i32, region_z: i32, map_size: usize) -> Vec<u32> {
    let half = (map_size >> 4) as i32;
    let mut out = Vec::new();
    for mx in (region_x - half) / 8..=(half + region_x) / 8 {
        for mz in (region_z - half) / 8..=(half + region_z) / 8 {
            out.push(((mx as u32) << 8) | mz as u32);
        }
    }
    out
}

/// Load the LAND/LOC/UNDERWATER_LAND files of every square in `packed`
/// that exists in the pack.
pub fn load_squares(pack: &Pack, packed: &[u32]) -> anyhow::Result<Vec<SquareFiles>> {
    rs910_core::profile::scope!("build load squares");
    let mut out = Vec::new();
    for &p in packed {
        let mx = p >> 8;
        let mz = p & 0xFF;
        let group = mx | (mz << 7);
        let files = match pack.read_group(map::MAP_ARCHIVE, group) {
            Ok(files) => files,
            Err(_) => continue,
        };
        let Some(land) = files.get(&cache::LAND_FILE) else {
            continue;
        };
        out.push(SquareFiles {
            packed: p,
            group,
            land: Some(land.clone()),
            loc: files.get(&cache::LOC_FILE).cloned(),
            underwater_land: files.get(&UNDERWATER_LAND_FILE).cloned(),
            underwater_loc: files.get(&UNDERWATER_LOC_FILE).cloned(),
        });
    }
    Ok(out)
}

/// The `readNormalLocs` admission filter
/// on a decoded loc, in scene-local coordinates.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "loc admission filter; exercised by tests only")
)]
fn admits_loc(loc: &crate::config::Loc, x: i32, z: i32, angle: u8, shape: u8, size: i32) -> bool {
    let (w, l) = if angle & 1 == 0 {
        (i32::from(loc.width), i32::from(loc.length))
    } else {
        (i32::from(loc.length), i32::from(loc.width))
    };
    if x >= size || z >= size {
        return false;
    }
    if x + w <= 0 || z + l <= 0 {
        return false;
    }
    if shape != 10 && shape != 11 && (x <= 0 || z <= 0 || x >= size - 1 || z >= size - 1) {
        return false;
    }
    true
}

/// Run the floor slice of a normal rebuild centred on absolute tile
/// `(centre_x, centre_z)`. `locs` enables the wall shade stamps.
pub fn rebuild_normal(
    pack: &Pack,
    flo: &FloTables,
    materials: &MaterialStore,
    locs: Option<&LocStore>,
    centre_x: i32,
    centre_z: i32,
    prefs: &BuildPrefs,
) -> anyhow::Result<Rebuild> {
    rs910_core::profile::scope!("scene rebuild normal");
    let map_size = MAP_SIZE_STANDARD;
    let region_x = centre_x >> 3;
    let region_z = centre_z >> 3;
    let base_x = (region_x - (map_size as i32 >> 4)) * 8;
    let base_z = (region_z - (map_size as i32 >> 4)) * 8;
    let squares = load_squares(pack, &window_squares(region_x, region_z, map_size))?;
    if squares.is_empty() {
        anyhow::bail!("no map squares in the pack around {centre_x},{centre_z}");
    }
    build_normal(
        BuildStores {
            pack,
            flo,
            materials,
            locs,
        },
        prefs,
        BuildWindow {
            map_size,
            base: [base_x, base_z],
            region_z,
        },
        squares,
        None,
    )
}

/// Renderer installation of the exact decoded
/// map request. Preserve allocated/trailing slots and missing LAND clear order.
/// This is distinct from the offline viewer's convenient centre-tile window.
pub fn rebuild_world(
    pack: &Pack,
    flo: &FloTables,
    materials: &MaterialStore,
    locs: &LocStore,
    world: &crate::protocol910::rebuild_state::World,
    prefs: &BuildPrefs,
) -> anyhow::Result<Rebuild> {
    rs910_core::profile::scope!("scene rebuild world");
    // The last rebuild kind stays CUTSCENE across the normal rebuilds that follow a cutscene,
    // while the rebuild itself is a normal rebuild.
    anyhow::ensure!(
        matches!(
            world.last_kind,
            crate::protocol910::rebuild_state::Kind::Normal
                | crate::protocol910::rebuild_state::Kind::Cutscene
        ),
        "normal scene rebuild required"
    );
    anyhow::ensure!(
        world.width == world.height && (1..=256).contains(&world.width),
        "invalid scene dimensions"
    );
    anyhow::ensure!(
        world.groups.len() == world.map_squares.len() && world.group_count <= world.groups.len(),
        "invalid map slot arrays"
    );
    let mut squares = Vec::with_capacity(world.groups.len());
    for (i, (&group, &packed)) in world.groups.iter().zip(&world.map_squares).enumerate() {
        let mut files = if i < world.group_count {
            pack.read_group(map::MAP_ARCHIVE, u32::try_from(group)?)?
        } else {
            Default::default()
        };
        squares.push(SquareFiles {
            group: u32::try_from(group)?,
            packed: packed as u32,
            land: files.remove(&cache::LAND_FILE),
            loc: files.remove(&cache::LOC_FILE),
            underwater_land: files.remove(&UNDERWATER_LAND_FILE),
            underwater_loc: files.remove(&UNDERWATER_LOC_FILE),
        });
    }
    build_normal(
        BuildStores {
            pack,
            flo,
            materials,
            locs: Some(locs),
        },
        prefs,
        BuildWindow {
            map_size: world.width as usize,
            base: [world.base_x, world.base_z],
            region_z: world.region_z,
        },
        squares,
        None,
    )
}

/// Build an instanced region using the source squares and destination chunk
/// templates decoded by `World.rebuildRegionMap`.
pub fn rebuild_region_world(
    pack: &Pack,
    flo: &FloTables,
    materials: &MaterialStore,
    locs: &LocStore,
    world: &crate::protocol910::rebuild_state::World,
    layout: &crate::protocol910::rebuild_state::RegionLayout,
    prefs: &BuildPrefs,
) -> anyhow::Result<Rebuild> {
    anyhow::ensure!(
        world.width == world.height && (1..=256).contains(&world.width),
        "invalid region dimensions"
    );
    anyhow::ensure!(
        layout.chunks_x * 8 == world.width as usize && layout.chunks_z * 8 == world.height as usize,
        "region template dimensions"
    );
    anyhow::ensure!(
        layout.templates.len() == 4 * layout.chunks_x * layout.chunks_z,
        "region template count"
    );
    let mut squares = Vec::with_capacity(world.groups.len());
    for (i, (&group, &packed)) in world.groups.iter().zip(&world.map_squares).enumerate() {
        let mut files = if i < world.group_count {
            pack.read_group(map::MAP_ARCHIVE, u32::try_from(group)?)?
        } else {
            Default::default()
        };
        squares.push(SquareFiles {
            group: u32::try_from(group)?,
            packed: packed as u32,
            land: files.remove(&cache::LAND_FILE),
            loc: files.remove(&cache::LOC_FILE),
            underwater_land: files.remove(&UNDERWATER_LAND_FILE),
            underwater_loc: files.remove(&UNDERWATER_LOC_FILE),
        });
    }
    build_normal(
        BuildStores {
            pack,
            flo,
            materials,
            locs: Some(locs),
        },
        prefs,
        BuildWindow {
            map_size: world.width as usize,
            base: [world.base_x, world.base_z],
            region_z: world.region_z,
        },
        squares,
        Some(layout),
    )
}

fn region_template_parts(template: i32) -> (usize, i32, i32, i32) {
    (
        ((template >> 24) & 0x3) as usize,
        (template >> 14) & 0x3ff,
        (template >> 3) & 0x7ff,
        (template >> 1) & 0x3,
    )
}

fn read_region_landscapes(
    loader: &mut MapLoader<'_>,
    flags: &mut SceneLevelTileFlags,
    env: &mut crate::env::EnvState,
    settings: crate::env::EnvironmentSettings<'_>,
    squares: &[SquareFiles],
    layout: &crate::protocol910::rebuild_state::RegionLayout,
    underwater: bool,
) -> anyhow::Result<()> {
    // World.readRegionLandscape: the loader's own
    // levels (1 underwater), source level 0 only underwater, the first
    // square with that id and a file; unset chunks are cleared afterwards.
    let levels = loader.levels;
    for level in 0..levels {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                let index = (level * layout.chunks_x + cx) * layout.chunks_z + cz;
                let template = layout.templates[index];
                let tile_x = (cx * 8) as i32;
                let tile_z = (cz * 8) as i32;
                if template < 0 {
                    continue;
                }
                let (src_level, src_chunk_x, src_chunk_z, rotation) =
                    region_template_parts(template);
                if underwater && src_level != 0 {
                    continue;
                }
                let packed = ((src_chunk_x as u32 >> 3) << 8) | (src_chunk_z as u32 >> 3);
                let land = squares
                    .iter()
                    .filter(|sq| sq.packed == packed)
                    .find_map(|sq| {
                        if underwater {
                            sq.underwater_land.as_ref()
                        } else {
                            sq.land.as_ref()
                        }
                    });
                let Some(land) = land else {
                    continue;
                };
                let mut packet = Packet::new(land);
                let copy = RegionCopy {
                    level,
                    tile_x,
                    tile_z,
                    src_level,
                    src_chunk_x,
                    src_chunk_z,
                    rotation,
                };
                loader
                    .read_region_landscape(&mut packet, flags, copy)
                    .with_context(|| {
                        format!("region square {packed:#06x} template {template:#x}")
                    })?;
                env.read_region_environment(
                    &mut packet,
                    copy,
                    &loader.level_heightmap,
                    settings,
                    underwater,
                )
                .with_context(|| {
                    format!("region environment {packed:#06x} template {template:#x}")
                })?;
            }
        }
    }
    for level in 0..levels {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                if layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz] == -1 {
                    loader.clear_landscape_level(level, (cx * 8) as i32, (cz * 8) as i32, 8, 8);
                }
            }
        }
    }
    Ok(())
}

/// Where a region chunk comes from and where it lands: the packed source
/// square, the source chunk, the destination chunk and the quarter turns
/// between them.
#[derive(Clone, Copy)]
struct ChunkMapping {
    source_square: u32,
    source_chunk: [i32; 2],
    destination_chunk: [usize; 2],
    rotation: i32,
}

fn rotate_region_loc(
    loc: crate::map::LocSpawn,
    chunk: ChunkMapping,
    store: &LocStore,
    base: [i32; 2],
    level: u8,
) -> Option<crate::map::LocSpawn> {
    let ChunkMapping {
        source_square,
        source_chunk: [src_chunk_x, src_chunk_z],
        destination_chunk: [dst_chunk_x, dst_chunk_z],
        rotation,
    } = chunk;
    let [base_x, base_z] = base;
    let source_base_x = ((source_square >> 8) as i32) * 64;
    let source_base_z = ((source_square & 0xff) as i32) * 64;
    let local_x = loc.x - source_base_x;
    let local_z = loc.z - source_base_z;
    let chunk_x = (src_chunk_x & 7) * 8;
    let chunk_z = (src_chunk_z & 7) * 8;
    if local_x < chunk_x || local_x >= chunk_x + 8 || local_z < chunk_z || local_z >= chunk_z + 8 {
        return None;
    }
    let config = store.get(loc.id)?;
    let (mut width, mut length) = (i32::from(config.width), i32::from(config.length));
    if loc.angle & 1 != 0 {
        std::mem::swap(&mut width, &mut length);
    }
    let x = local_x - chunk_x;
    let z = local_z - chunk_z;
    let rx = match rotation & 3 {
        0 => x,
        1 => z,
        2 => 7 - x - (width - 1),
        _ => 7 - z - (length - 1),
    };
    let rz = match rotation & 3 {
        0 => z,
        1 => 7 - x - (width - 1),
        2 => 7 - z - (length - 1),
        _ => x,
    };
    Some(crate::map::LocSpawn {
        id: loc.id,
        level,
        x: base_x + (dst_chunk_x as i32 * 8) + rx,
        z: base_z + (dst_chunk_z as i32 * 8) + rz,
        shape: loc.shape,
        angle: crate::map::rotate_loc_angle(loc.angle, rotation as u8),
        srt: loc.srt,
    })
}

/// Region loc read over the normal loader's `LOC` files, or the underwater loader's
/// `UNDERWATER_LOC` files: all source levels for the normal loader, only source level 0 for
/// the underwater one.
fn decode_region_locs(
    squares: &[SquareFiles],
    layout: &crate::protocol910::rebuild_state::RegionLayout,
    store: &LocStore,
    base_x: i32,
    base_z: i32,
    underwater: bool,
) -> anyhow::Result<Vec<crate::map::LocSpawn>> {
    let mut decoded = HashMap::new();
    for square in squares {
        let file = if underwater {
            &square.underwater_loc
        } else {
            &square.loc
        };
        if let Some(bytes) = file {
            let x = ((square.packed >> 8) as i32) * 64;
            let z = ((square.packed & 0xff) as i32) * 64;
            decoded.insert(square.packed, map::decode_locs(bytes, x, z)?);
        }
    }
    let mut out = Vec::new();
    let levels = if underwater { 1 } else { 4 };
    for level in 0..levels {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                let template =
                    layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz];
                if template < 0 {
                    continue;
                }
                let (src_level, src_chunk_x, src_chunk_z, rotation) =
                    region_template_parts(template);
                if underwater && src_level != 0 {
                    continue;
                }
                let packed = ((src_chunk_x as u32 >> 3) << 8) | (src_chunk_z as u32 >> 3);
                let Some(locs) = decoded.get(&packed) else {
                    continue;
                };
                for &loc in locs {
                    if loc.level != src_level as u8 {
                        continue;
                    }
                    let chunk = ChunkMapping {
                        source_square: packed,
                        source_chunk: [src_chunk_x, src_chunk_z],
                        destination_chunk: [cx, cz],
                        rotation,
                    };
                    if let Some(mapped) =
                        rotate_region_loc(loc, chunk, store, [base_x, base_z], level as u8)
                    {
                        out.push(mapped);
                    }
                }
            }
        }
    }
    Ok(out)
}

/// The cache stores a scene build reads. `locs` is absent for a floor-only
/// build.
#[derive(Clone, Copy)]
struct BuildStores<'a> {
    pack: &'a Pack,
    flo: &'a FloTables,
    materials: &'a MaterialStore,
    locs: Option<&'a LocStore>,
}

/// The window a build covers: its size in tiles, its base tile and the map
/// row the missing-square clearing rule looks at.
#[derive(Clone, Copy)]
struct BuildWindow {
    map_size: usize,
    base: [i32; 2],
    region_z: i32,
}

fn build_normal(
    stores: BuildStores<'_>,
    prefs: &BuildPrefs,
    window: BuildWindow,
    squares: Vec<SquareFiles>,
    region: Option<&crate::protocol910::rebuild_state::RegionLayout>,
) -> anyhow::Result<Rebuild> {
    let BuildStores {
        pack,
        flo,
        materials,
        locs,
    } = stores;
    let BuildWindow {
        map_size,
        base: [base_x, base_z],
        region_z,
    } = window;
    rs910_core::profile::scope!("scene build");
    let start = Instant::now();
    // An underwater LOC or LAND file enables the pass.
    let has_underwater = prefs.water_detail == 2
        && squares
            .iter()
            .any(|s| s.underwater_land.is_some() || s.underwater_loc.is_some());

    let mut flags = SceneLevelTileFlags::new(4, map_size, map_size);
    let mut scene = SceneFloors::new(4, map_size, map_size, has_underwater);
    let mut loader = MapLoader::new(flo, materials, 4, map_size, map_size, false);
    prefs.apply(&mut loader);

    // 1333 readNormalLandscape + readNormalEnvironment from the same
    // packet. Region templates use the rotated landscape and
    // environment readers, preserving destination chunk transforms.
    let light_types = crate::env::LightTypeStore::load(pack).context("load light types")?;
    let mut env = crate::env::EnvState::new(map_size, map_size);
    let settings = crate::env::EnvironmentSettings {
        light_types: &light_types,
        max_lights: crate::env::GLX_MAX_LIGHTS,
        sun_enabled: prefs.lighting_detail == 1 && crate::env::GLX_MAX_LIGHTS > 0,
    };
    if let Some(layout) = region {
        read_region_landscapes(
            &mut loader,
            &mut flags,
            &mut env,
            settings,
            &squares,
            layout,
            false,
        )?;
    } else {
        for sq in &squares {
            let Some(land) = &sq.land else { continue };
            let mut packet = Packet::new(land);
            let tx = (sq.packed >> 8) as i32 * 64 - base_x;
            let tz = (sq.packed & 0xFF) as i32 * 64 - base_z;
            loader
                .read_normal_landscape(&mut packet, &mut flags, tx, tz, base_x, base_z)
                .with_context(|| format!("square {:#06x} (group {})", sq.packed, sq.group))?;
            env.read_normal_environment(
                &mut packet,
                [tx, tz],
                &loader.level_heightmap,
                settings,
                false,
            )
            .with_context(|| format!("environment trailer of square {:#06x}", sq.packed))?;
        }
    }
    // Missing squares are cleared only after all existing LAND slots have loaded. Copying an edge earlier observes different neighbours.
    if region.is_none() && region_z < 800 {
        for sq in &squares {
            if sq.land.is_none() {
                loader.clear_landscape(
                    (sq.packed >> 8) as i32 * 64 - base_x,
                    (sq.packed & 255) as i32 * 64 - base_z,
                    64,
                    64,
                );
            }
        }
    }
    // The environment manager adopts the centre chunk's sun direction and pushes the sun
    // update. At this point the current environment is still the default (no partial update
    // has run for the new map), so the sun is the default intensities with the window's
    // direction. Only the direction reaches the build (hard-shadow offsets).
    env.adopt_centre_sun_direction();
    let sun =
        crate::env::Environment::default().sun_lighting(env.sun_direction, prefs.brightness, 0.0);
    log::info!(
        "[rebuild] sun direction {:?} -> shadow offsets {},{}",
        env.sun_direction,
        sun.shadow_offset_x,
        sun.shadow_offset_z
    );

    // 1146 underwater pass.
    let mut underwater_loader = if has_underwater {
        let mut uw = MapLoader::new(flo, materials, 1, map_size, map_size, true);
        prefs.apply(&mut uw);
        if let Some(layout) = region {
            read_region_landscapes(
                &mut uw, &mut flags, &mut env, settings, &squares, layout, true,
            )?;
        } else {
            for sq in &squares {
                let tx = (sq.packed >> 8) as i32 * 64 - base_x;
                let tz = (sq.packed & 0xFF) as i32 * 64 - base_z;
                match &sq.underwater_land {
                    Some(bytes) => {
                        let mut packet = Packet::new(bytes);
                        uw.read_normal_landscape(&mut packet, &mut flags, tx, tz, base_x, base_z)
                            .with_context(|| format!("underwater square {:#06x}", sq.packed))?;
                    }
                    None => uw.clear_landscape(tx, tz, 64, 64),
                }
            }
        }
        uw.add_heightmap(0, &loader.level_heightmap[0]);
        uw.build_floors(&mut scene, None);
        Some(uw)
    } else {
        None
    };

    // Build the floors.
    loader.build_floors(
        &mut scene,
        underwater_loader
            .as_ref()
            .map(|uw| uw.level_heightmap.as_slice()),
    );

    // The full loc placement into the scene graph, which also stamps the floor shade
    // map under walls and centrepieces.
    let mut all_locs = Vec::new();
    let mut scene_graph: Option<Scene> = None;
    let mut loc_stats = LocStats::default();
    let mut loc_sounds = crate::loc_sound::LocSoundScene::default();
    let (billboards, emitters) = if locs.is_some() {
        (
            crate::billboard::BillboardStore::load(pack).context("load billboard types")?,
            crate::particle::EmitterStore::load(pack).context("load particle emitter types")?,
        )
    } else {
        (
            crate::billboard::BillboardStore::default(),
            crate::particle::EmitterStore::default(),
        )
    };
    let model_source = ModelSource::new(
        pack,
        materials,
        &billboards,
        &emitters,
        prefs.model_detail(),
    );
    if let Some(store) = locs {
        if let Some(layout) = region {
            all_locs = decode_region_locs(&squares, layout, store, base_x, base_z, false)?;
        } else {
            for sq in &squares {
                let Some(bytes) = &sq.loc else { continue };
                let gx = (sq.packed >> 8) as i32 * 64;
                let gz = (sq.packed & 0xFF) as i32 * 64;
                let spawns = map::decode_locs(bytes, gx, gz)
                    .with_context(|| format!("locs of square {:#06x}", sq.packed))?;
                all_locs.extend(spawns);
            }
        }
        let mut graph = Scene::new(9, 4, map_size, map_size);
        {
            let mut placer = LocPlacer {
                scene: &mut graph,
                floors: &mut scene.normal_builders,
                level_occludemap: &mut loader.level_occludemap,
                locs: store,
                models: &model_source,
                prefs: prefs.placement(),
                is_blending: loader.is_blending,
                underwater: false,
                levels: 4,
                min_level: 99,
                stats: LocStats::default(),
                loc_tint: (0, 0, 0, 0),
                sun,
                sounds: Vec::new(),
            };
            // Normal squares are read in the rebuild's square order. Region
            // locs are already transformed into destination coordinates.
            if region.is_some() {
                placer.read_normal_locs(&all_locs, base_x, base_z)?;
            } else {
                for sq in &squares {
                    let gx = (sq.packed >> 8) as i32 * 64;
                    let gz = (sq.packed & 0xFF) as i32 * 64;
                    let mine: Vec<LocSpawn> = all_locs
                        .iter()
                        .copied()
                        .filter(|s| s.x >= gx && s.x < gx + 64 && s.z >= gz && s.z < gz + 64)
                        .collect();
                    placer.read_normal_locs(&mine, base_x, base_z)?;
                }
            }
            loc_stats = placer.stats;
            loc_sounds = crate::loc_sound::LocSoundScene {
                spawns: placer.sounds,
                table: std::sync::Arc::new(crate::loc_sound::LocSoundTable::from_store(store)),
            };
        }
        scene_graph = Some(graph);
    }

    // Build the normal ground, then the underwater ground.
    let underwater_floor0 = scene
        .underwater_builders
        .first()
        .and_then(|b| b.as_ref())
        .map(|b| b.heights.clone());
    let lighting = scene_graph
        .as_ref()
        .map(|graph| (env.lights.as_slice(), graph));
    loader.build_ground(
        &mut scene,
        &flags,
        underwater_floor0.as_ref(),
        None,
        &sun,
        lighting,
    )?;
    let mut underwater_models = Vec::new();
    if let Some(uw) = underwater_loader.as_mut() {
        // Place the underwater LOC files into the underwater scene: one level.
        let mut graph = locs.map(|_| Scene::new(9, 1, map_size, map_size));
        if let (Some(graph), Some(store)) = (graph.as_mut(), locs) {
            let mut placer = LocPlacer {
                scene: graph,
                floors: &mut scene.underwater_builders,
                level_occludemap: &mut uw.level_occludemap,
                locs: store,
                models: &model_source,
                prefs: prefs.placement(),
                is_blending: uw.is_blending,
                underwater: true,
                levels: 1,
                min_level: 99,
                stats: LocStats::default(),
                loc_tint: (0, 0, 0, 0),
                sun,
                sounds: Vec::new(),
            };
            if let Some(layout) = region {
                // Region locs for the underwater loader.
                let spawns = decode_region_locs(&squares, layout, store, base_x, base_z, true)?;
                placer.read_normal_locs(&spawns, base_x, base_z)?;
            } else {
                for sq in &squares {
                    let Some(bytes) = &sq.underwater_loc else {
                        continue;
                    };
                    let gx = (sq.packed >> 8) as i32 * 64;
                    let gz = (sq.packed & 0xFF) as i32 * 64;
                    let spawns = map::decode_locs(bytes, gx, gz)
                        .with_context(|| format!("underwater locs of square {:#06x}", sq.packed))?;
                    placer.read_normal_locs(&spawns, base_x, base_z)?;
                }
            }
            // Background sounds are registered for the underwater loader too (no underwater
            // check).
            loc_sounds.spawns.extend(placer.sounds);
        }
        let normal0 = scene.normal[0].as_ref().map(|g| g.heights.clone());
        uw.build_ground(&mut scene, &flags, None, normal0.as_ref(), &sun, None)?;
        // The underwater loader's build: model building only.
        if let Some(graph) = graph.as_mut() {
            let heights: Vec<crate::floor::FloorHeights> = scene
                .underwater
                .iter()
                .flatten()
                .map(|g| g.heights.clone())
                .collect();
            if !heights.is_empty() {
                graph.build_models(&heights);
            }
            if let (Some(fog), Some(normal0)) = (scene.water_fog.as_ref(), normal0.as_ref()) {
                underwater_models = underwater_models_of(graph, fog, normal0);
            }
        }
        uw.disable_blending();
    }
    loader.disable_blending();

    // The normal loader's build (model building, bridges, occluders), then the scene reset.
    // The ground build's tile allocations are replayed from the tile marks first (they
    // precede the build).
    if let (Some(graph), Some(store)) = (scene_graph.as_mut(), locs) {
        for level in 0..4 {
            for x in 0..map_size {
                for z in 0..map_size {
                    if scene.normal_tiles.has(level, x, z) && graph.tile(level, x, z).is_none() {
                        graph.create_tile(level, x, z);
                    }
                }
            }
        }
        let heights: Vec<crate::floor::FloorHeights> = scene
            .normal
            .iter()
            .map(|g| {
                g.as_ref()
                    .map(|g| g.heights.clone())
                    .ok_or_else(|| anyhow::anyhow!("floor missing"))
            })
            .collect::<anyhow::Result<_>>()?;
        let mut placer = LocPlacer {
            scene: graph,
            floors: &mut scene.normal_builders,
            level_occludemap: &mut loader.level_occludemap,
            locs: store,
            models: &model_source,
            prefs: prefs.placement(),
            is_blending: false,
            underwater: false,
            levels: 4,
            min_level: 99,
            stats: loc_stats,
            loc_tint: (0, 0, 0, 0),
            sun,
            sounds: Vec::new(),
        };
        placer.build(&flags, &heights);
        graph.reset();
    }

    // The retained calls carry the sun the floor build saw, for a renderer that builds its
    // floors from them.
    for geometry in scene
        .normal
        .iter_mut()
        .chain(scene.underwater.iter_mut())
        .flatten()
    {
        if let Some(calls) = geometry.calls.as_mut() {
            std::sync::Arc::make_mut(calls).sun = Some(sun);
        }
    }
    let lights = std::mem::take(&mut scene.lights);
    Ok(Rebuild {
        occludemap: loader.level_occludemap.clone(),
        scene,
        flags,
        base_x,
        base_z,
        map_size,
        squares: squares.iter().map(|s| s.packed).collect(),
        locs: all_locs,
        scene_graph,
        underwater_models,
        loc_stats,
        loc_sounds,
        model_ids: model_source.loaded_ids(),
        model_cache: model_source.model_cache.clone(),
        env,
        lights,
        elapsed_ms: start.elapsed().as_millis(),
    })
}

/// Write the raw js5 containers needed to rebuild
/// the same window elsewhere: `<dir>/<archive>/index.bin`
/// (master container) and `<dir>/<archive>/group_<id>.bin` for the flo
/// config groups (1, 4), the materials and billboards groups (0), the
/// particle emitter/effector groups (0, 1), every
/// loaded map square, every `loc.config` group and the `models` groups in
/// `model_ids`.
pub fn dump_raw_inputs(
    pack: &Pack,
    dir: &Path,
    squares: &[u32],
    model_ids: &[u32],
) -> anyhow::Result<()> {
    let mut wanted: Vec<(&str, Vec<u32>)> = vec![
        (
            "config",
            vec![
                crate::flo::UNDERLAY_GROUP,
                crate::flo::OVERLAY_GROUP,
                crate::env::LIGHTTYPE_GROUP,
            ],
        ),
        ("materials", vec![0]),
        ("billboards", vec![0]),
        // Group 0 emitter types, group 1 effector types.
        ("particles", vec![0, 1]),
    ];
    let groups: Vec<u32> = squares
        .iter()
        .map(|p| (p >> 8) | ((p & 0xFF) << 7))
        .collect();
    wanted.push((map::MAP_ARCHIVE, groups));
    if !model_ids.is_empty() {
        let loc_index = pack.read_archive_index(crate::config::LOC_ARCHIVE)?;
        wanted.push((crate::config::LOC_ARCHIVE, loc_index.group_id.clone()));
        wanted.push((crate::modelunlit::MODEL_ARCHIVE, model_ids.to_vec()));
    }
    for (archive, groups) in wanted {
        let sub = dir.join(archive);
        std::fs::create_dir_all(&sub)?;
        std::fs::write(sub.join("index.bin"), pack.read_raw_master(archive)?)?;
        for g in groups {
            let bytes = pack
                .read_raw_group(archive, g)
                .with_context(|| format!("raw {archive} group {g}"))?;
            std::fs::write(sub.join(format!("group_{g}.bin")), bytes)?;
        }
    }
    log::info!("[rebuild] raw js5 inputs written to {}", dir.display());
    Ok(())
}

/// Print a per-level summary and, when `dump_dir` is given, write
/// `floor_L{n}.bin` (`FloorGeometry::to_dump`) per level and `scene.bin`
/// (`Scene::to_dump`) for diffing against a recorded capture.
pub fn report(result: &Rebuild, dump_dir: Option<&Path>) -> anyhow::Result<()> {
    report_with(result, dump_dir, None)
}

/// [`report`] with the material store needed to serialise the scene graph.
pub fn report_with(
    result: &Rebuild,
    dump_dir: Option<&Path>,
    materials: Option<&MaterialStore>,
) -> anyhow::Result<()> {
    if let (Some(dir), Some(graph), Some(materials)) = (dump_dir, &result.scene_graph, materials) {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("scene.bin");
        std::fs::write(&path, graph.to_dump(materials)?)
            .with_context(|| format!("write {}", path.display()))?;
        log::info!("[rebuild] wrote {}", path.display());
    }
    if let Some(dir) = dump_dir {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("env.bin");
        std::fs::write(&path, result.env.to_dump())
            .with_context(|| format!("write {}", path.display()))?;
        log::info!("[rebuild] wrote {}", path.display());
    }
    if let Some(dir) = dump_dir {
        for (level, baked) in result.lights.iter().enumerate() {
            let path = dir.join(format!("lights_L{level}.bin"));
            std::fs::write(&path, crate::floorlight::to_dump(baked))
                .with_context(|| format!("write {}", path.display()))?;
        }
    }
    log::info!(
        "[rebuild] baked lights per level: {:?} (vertices {:?})",
        result.lights.iter().map(Vec::len).collect::<Vec<_>>(),
        result
            .lights
            .iter()
            .map(|l| l.iter().map(|b| b.vertex_count()).sum::<usize>())
            .collect::<Vec<_>>()
    );
    log::info!(
        "[rebuild] env: {} chunks set, {} static lights, camera offsets {}, force-ground {}",
        result.env.map.iter().filter(|e| e.is_some()).count(),
        result.env.lights.len(),
        result.env.camera_height.is_some(),
        result.env.force_ground.iter().filter(|b| **b).count()
    );
    log::info!(
        "[rebuild] base {},{} size {} squares {:?} locs {} placed {} (dynamic placeholders {}) model-failures {} srt {} (non-identity {}) in {} ms",
        result.base_x,
        result.base_z,
        result.map_size,
        result.squares,
        result.locs.len(),
        result.loc_stats.placed,
        result.loc_stats.dynamic_placed,
        result.loc_stats.model_failures,
        result.loc_stats.srt_locs,
        result.loc_stats.srt_nonidentity,
        result.elapsed_ms
    );
    if let Some(graph) = &result.scene_graph {
        log::info!(
            "[rebuild] scene: scenery {} walls {} wall decors {} ground decors {} | opaque {} transparent {} pending {} | occlude calls {} occluders {}",
            graph.scenery.len(),
            graph.walls.len(),
            graph.wall_decors.len(),
            graph.ground_decors.len(),
            graph.opaque.len(),
            graph.transparent.len(),
            graph.pending.len(),
            graph.occlude_calls.len(),
            graph.occluders.len()
        );
    }
    for (level, geo) in result.scene.normal.iter().enumerate() {
        let Some(geo) = geo else {
            log::info!("[rebuild] level {level}: no floor");
            continue;
        };
        let tiles = geo.tile_tris.iter().filter(|t| t.is_some()).count();
        let materials: std::collections::BTreeSet<i32> =
            geo.batches.iter().map(|b| b.material).collect();
        log::info!(
            "[rebuild] level {level}: tiles {} verts {} tris {} batches {} (materials {}) stride {} depth {} normals {} y {}..{}",
            tiles,
            geo.vertex_count,
            geo.triangle_count(),
            geo.batches.len(),
            materials.len(),
            geo.stride_floats,
            geo.has_depth,
            geo.has_normals,
            geo.min_y,
            geo.max_y,
        );
        if let Some(dir) = dump_dir {
            std::fs::create_dir_all(dir)?;
            let path = dir.join(format!("floor_L{level}.bin"));
            std::fs::write(&path, geo.to_dump())
                .with_context(|| format!("write {}", path.display()))?;
            log::info!("[rebuild] wrote {}", path.display());
        }
    }
    if let Some(Some(geo)) = result.scene.underwater.first() {
        log::info!(
            "[rebuild] underwater: verts {} tris {} batches {}",
            geo.vertex_count,
            geo.triangle_count(),
            geo.batches.len()
        );
        if let Some(dir) = dump_dir {
            std::fs::write(dir.join("floor_underwater.bin"), geo.to_dump())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lumbridge_window() {
        // Spawn 3222,3222: zone 402 -> base (402-6)*8 = 3168, squares 49..51.
        let squares = window_squares(3222 >> 3, 3222 >> 3, MAP_SIZE_STANDARD);
        assert_eq!(squares.len(), 9);
        assert_eq!(squares[0], (49 << 8) | 49);
        assert_eq!(squares[8], (51 << 8) | 51);
        assert_eq!((402 - (104 >> 4)) * 8, 3168);
    }

    /// Underwater region locs: one destination level, source level
    /// 0 only, from the squares' UNDERWATER_LOC files.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn region_underwater_locs_follow_read_region_locs() {
        let pack = crate::test_support::require_pack("client.mapsv2.js5");
        let locs = LocStore::load(&pack).unwrap();
        let index = pack.read_archive_index(map::MAP_ARCHIVE).unwrap();
        let (group, bytes, spawns) = index
            .group_id
            .iter()
            .find_map(|&group| {
                let files = pack.read_group(map::MAP_ARCHIVE, group).ok()?;
                let bytes = files.get(&UNDERWATER_LOC_FILE)?.clone();
                let (mx, mz) = ((group & 0x7F) as i32, (group >> 7) as i32);
                let spawns = map::decode_locs(&bytes, mx * 64, mz * 64).ok()?;
                spawns
                    .iter()
                    .any(|s| s.level == 0)
                    .then_some((group, bytes, spawns))
            })
            .expect("an underwater loc square");
        let (mx, mz) = ((group & 0x7F) as i32, (group >> 7) as i32);
        let spawn = spawns.iter().find(|s| s.level == 0).unwrap();
        // The source chunk of that loc, in absolute chunk units.
        let (src_cx, src_cz) = (spawn.x >> 3, spawn.z >> 3);
        let template = |src_level: i32| (src_level << 24) | (src_cx << 14) | (src_cz << 3);
        let chunks = 13;
        let mut templates = vec![-1; 4 * chunks * chunks];
        templates[0] = template(0); // level 0, chunk (0, 0): read.
        templates[1] = template(1); // level 0, chunk (0, 1): source level 1, skipped.
        templates[chunks * chunks + 2] = template(0); // level 1: beyond the loader's one level.
        let layout = crate::protocol910::rebuild_state::RegionLayout {
            chunks_x: chunks,
            chunks_z: chunks,
            templates,
        };
        let square = SquareFiles {
            group,
            packed: ((mx as u32) << 8) | mz as u32,
            land: None,
            loc: None,
            underwater_land: None,
            underwater_loc: Some(bytes),
        };
        let (base_x, base_z) = (6400, 6400);
        let out = decode_region_locs(&[square], &layout, &locs, base_x, base_z, true).unwrap();
        let expected = spawns
            .iter()
            .filter(|s| s.level == 0 && s.x >> 3 == src_cx && s.z >> 3 == src_cz)
            .count();
        assert!(expected > 0);
        assert_eq!(out.len(), expected);
        assert!(out.iter().all(|s| s.level == 0
            && (base_x..base_x + 8).contains(&s.x)
            && (base_z..base_z + 8).contains(&s.z)));
        // The normal loader reads LOC files, which this square fixture lacks.
        let square = SquareFiles {
            group,
            packed: ((mx as u32) << 8) | mz as u32,
            land: None,
            loc: None,
            underwater_land: None,
            underwater_loc: None,
        };
        assert!(
            decode_region_locs(&[square], &layout, &locs, base_x, base_z, false)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn loc_admission_filter() {
        let mut loc = crate::config::decode_loc(1, &[0]).unwrap();
        loc.width = 2;
        loc.length = 1;
        // Non-centrepiece on the border is rejected, centrepiece is not.
        assert!(!admits_loc(&loc, 0, 5, 0, 0, 104));
        assert!(admits_loc(&loc, 0, 5, 0, 10, 104));
        // Entirely off the west edge (x + w <= 0).
        assert!(!admits_loc(&loc, -2, 5, 0, 10, 104));
        assert!(admits_loc(&loc, -1, 5, 0, 10, 104));
        // Rotation swaps the footprint.
        assert!(admits_loc(&loc, -1, 5, 0, 10, 104));
        assert!(!admits_loc(&loc, -1, 5, 1, 10, 104));
        assert!(!admits_loc(&loc, 104, 5, 0, 10, 104));
    }
}
