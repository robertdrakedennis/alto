//! Loc placement: the normal loc read, ground/wall/wall-decoration placement, the scene
//! build, and the four static entity constructors.
//!
//! Dynamic locs (animated, multiloc, `forceDynamic`, `alwaysDynamic`) are placed as
//! model-less dynamic entity placeholders (counted in [`LocStats::dynamic_placed`]) whose
//! model, animation and shadow `dynamic_scene.rs` owns from the first frame.
//! Positional-sound registrations are recorded in [`LocPlacer::sounds`] for the audio owner.
//! Runtime loc changes use [`remove_loc_occluders`] / [`add_loc_occluders`] for the
//! occlusion side.

use crate::config::{Loc, LocStore};
use crate::floor::{FloorBuilder, FloorHeights};
use crate::loctype::{self, shape, ModelSource};
use crate::map::LocSpawn;
use crate::scene::{
    GroundDecorEntity, OccludeMapCall, OccluderRect, Scene, SceneryEntity, WallDecorEntity,
    WallEntity,
};
use crate::tileflags::SceneLevelTileFlags;

/// `WALL_DECORATION_ROTATION_FORWARD_X/Z`, `WALL_DECORATION_OFFSET_X/Z`
const ROTATION_FORWARD_X: [i32; 4] = [1, 0, -1, 0];
const ROTATION_FORWARD_Z: [i32; 4] = [0, -1, 0, 1];
const OFFSET_X: [i32; 4] = [1, -1, -1, 1];
const OFFSET_Z: [i32; 4] = [-1, -1, 1, 1];
/// Wall side and corner type bits by rotation.
const WALL_TYPE: [i32; 4] = [1, 2, 4, 8];
const WALL_CORNER_TYPE: [i32; 4] = [16, 32, 64, 128];

/// The client preferences the placement reads.
#[derive(Clone, Copy, Debug)]
pub struct PlacePrefs {
    /// `textures` (default 1).
    pub textures: i32,
    /// `groundDecoration` (default 1).
    pub ground_decoration: i32,
    /// `animDetail` (default 1).
    pub anim_detail: i32,
    /// `sceneryShadows` (default 2).
    pub scenery_shadows: i32,
}

impl Default for PlacePrefs {
    fn default() -> Self {
        Self {
            textures: 1,
            ground_decoration: 1,
            anim_detail: 1,
            scenery_shadows: 2,
        }
    }
}

/// Counters of what the placement did / skipped.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocStats {
    pub placed: usize,
    /// Dynamic locs placed as model-less placeholders (dynamic entities;
    /// `dynamic_scene.rs` supplies their models).
    pub dynamic_placed: usize,
    pub model_failures: usize,
    pub srt_locs: usize,
    /// `srt_locs` whose transform is not the identity.
    pub srt_nonidentity: usize,
    /// Static entities whose hard shadow was stamped.
    pub entity_shadows: usize,
}

/// State the loc placement methods use.
pub struct LocPlacer<'a> {
    pub scene: &'a mut Scene,
    /// `scene.levelHeightmaps` before finalisation (normal set).
    pub floors: &'a mut [Option<FloorBuilder>],
    /// Per-level occlusion map, indexed `[level][x * (maxZ + 1) + z]`.
    pub level_occludemap: &'a mut [Vec<u8>],
    pub locs: &'a LocStore,
    pub models: &'a ModelSource<'a>,
    pub prefs: PlacePrefs,
    /// `isBlending`.
    pub is_blending: bool,
    /// `underwater`.
    pub underwater: bool,
    /// `levels`.
    pub levels: usize,
    /// `minLevel`.
    pub min_level: i32,
    pub stats: LocStats,
    /// The world's loc tint (`(0,0,0,0)` until randomised).
    pub loc_tint: (i32, i32, i32, i32),
    /// The sun state set before the rebuild: the hard-shadow projection offsets.
    pub sun: crate::floor::SunLighting,
    /// Positional-sound registrations (level, x, z, angle, loc) in placement order.
    pub sounds: Vec<crate::loc_sound::LocSoundSpawn>,
}

/// A loc to place: the floor the model follows and the scene level it is
/// added to, the tile, the loc id, angle and shape, and its scale/rotation.
#[derive(Clone, Copy)]
struct LocRequest {
    floor_level: i32,
    scene_level: i32,
    tile: [i32; 2],
    loc_id: u32,
    angle: i32,
    shape: i32,
    srt: Option<crate::map::LocSrt>,
}

/// One loc being placed, as the wall and wall decoration placers see it:
/// the loc type, its shape and angle, whether it is static (else a
/// placeholder), its levels, its tile, its centre in scene fine units
/// (x, height, z), the merged-normals switch, its footprint in tiles and its
/// scale/rotation.
#[derive(Clone, Copy)]
struct Placement<'a> {
    loc_type: &'a Loc,
    loc_shape: i32,
    loc_angle: i32,
    is_static: bool,
    scene_level: i32,
    floor_level: i32,
    world: [i32; 3],
    tile: [i32; 2],
    merged: bool,
    size: [i32; 2],
    srt: Option<crate::map::LocSrt>,
}

/// What every entity constructor shares: the loc, its scene level and floor
/// level, the entity position, shape and angle, the merged-normals switch,
/// its scale/rotation, and whether the loc is static (else the entity is a
/// model-less placeholder that the dynamic scene fills in).
#[derive(Clone, Copy)]
struct EntitySpec<'a> {
    loc: &'a Loc,
    level: i32,
    occlude_level: i32,
    at: [i32; 3],
    shape: i32,
    angle: i32,
    merged: bool,
    srt: Option<crate::map::LocSrt>,
    is_static: bool,
}

/// Where a loc stands: its tiles, turned by its angle, and the centre of
/// that footprint in fine units at the mean floor height under the centre.
/// Locs the map places and locs the server adds while the client runs stand
/// the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Footprint {
    /// The minimum tile.
    pub tile: [i32; 2],
    /// Tiles along x and z after the turn.
    pub size: [i32; 2],
    /// `[x, height, z]` in fine units.
    pub centre: [i32; 3],
}

impl Footprint {
    /// The footprint of `loc` placed as `shape` turned by `angle` from `tile`
    /// on a scene `max` tiles wide, standing on `heights`. A centrepiece reads
    /// its heights clamped into the grid; the other shapes stand inside it.
    /// A footprint that would leave the scene samples its minimum tile.
    pub fn new(
        loc: &Loc,
        [shape, angle]: [i32; 2],
        tile: [i32; 2],
        heights: &FloorHeights,
        max: [i32; 2],
    ) -> Self {
        let [tile_x, tile_z] = tile;
        let (size_x, size_z) = if angle == 1 || angle == 3 {
            (i32::from(loc.length), i32::from(loc.width))
        } else {
            (i32::from(loc.width), i32::from(loc.length))
        };
        let (sample_x0, sample_x1) = if tile_x + size_x <= max[0] {
            ((size_x >> 1) + tile_x, ((size_x + 1) >> 1) + tile_x)
        } else {
            (tile_x, tile_x + 1)
        };
        let (sample_z0, sample_z1) = if tile_z + size_z <= max[1] {
            ((size_z >> 1) + tile_z, ((size_z + 1) >> 1) + tile_z)
        } else {
            (tile_z, tile_z + 1)
        };
        let height = if shape::CENTREPIECE_STRAIGHT == shape || shape::CENTREPIECE_DIAGONAL == shape
        {
            (heights.get_tile_height_clamped(sample_x0, sample_z0)
                + heights.get_tile_height_clamped(sample_x1, sample_z0)
                + heights.get_tile_height_clamped(sample_x0, sample_z1)
                + heights.get_tile_height_clamped(sample_x1, sample_z1))
                >> 2
        } else {
            (heights.get_tile_height(sample_x0 as usize, sample_z0 as usize)
                + heights.get_tile_height(sample_x1 as usize, sample_z0 as usize)
                + heights.get_tile_height(sample_x0 as usize, sample_z1 as usize)
                + heights.get_tile_height(sample_x1 as usize, sample_z1 as usize))
                >> 2
        };
        Self {
            tile,
            size: [size_x, size_z],
            centre: [
                (tile_x << 9) + (size_x << 8),
                height,
                (tile_z << 9) + (size_z << 8),
            ],
        }
    }

    /// The tiles it covers, `[min_x, max_x, min_z, max_z]`.
    pub fn bounds(&self) -> [i32; 4] {
        let [x, z] = self.tile;
        [x, x + self.size[0] - 1, z, z + self.size[1] - 1]
    }
}

/// Whether the loc, or any loc it can become, has a background sound.
fn has_background_sound(store: &LocStore, loc: &Loc) -> bool {
    let own = |loc: &Loc| loc.bgsound_sound != -1 || loc.bgsound_random.is_some();
    if !loc.has_multiloc {
        return own(loc);
    }
    loc.multiloc.iter().any(|&id| {
        id != -1
            && u32::try_from(id)
                .ok()
                .and_then(|id| store.get(id))
                .is_some_and(own)
    })
}

impl<'a> LocPlacer<'a> {
    fn max_x(&self) -> i32 {
        self.scene.max_x as i32
    }

    fn max_z(&self) -> i32 {
        self.scene.max_z as i32
    }

    fn heights(&self, level: usize) -> Option<&FloorHeights> {
        self.floors
            .get(level)
            .and_then(|f| f.as_ref())
            .map(|f| &f.heights)
    }

    /// Place pre-decoded spawns given in absolute coordinates.
    pub fn read_normal_locs(
        &mut self,
        spawns: &[LocSpawn],
        base_x: i32,
        base_z: i32,
    ) -> anyhow::Result<()> {
        rs910_core::profile::scope!("build place locs");
        for s in spawns {
            let tile_x = s.x - base_x;
            let tile_z = s.z - base_z;
            let Some(loc_type) = self.locs.get(s.id) else {
                continue;
            };
            let angle = i32::from(s.angle);
            let (size_x, size_z) = if (angle & 0x1) == 0 {
                (i32::from(loc_type.width), i32::from(loc_type.length))
            } else {
                (i32::from(loc_type.length), i32::from(loc_type.width))
            };
            let end_x = tile_x + size_x;
            let end_z = tile_z + size_z;
            if tile_x >= self.max_x() || tile_z >= self.max_z() || end_x <= 0 || end_z <= 0 {
                continue;
            }
            let sh = i32::from(s.shape);
            if shape::CENTREPIECE_STRAIGHT != sh
                && shape::CENTREPIECE_DIAGONAL != sh
                && (tile_x <= 0
                    || tile_z <= 0
                    || tile_x >= self.max_x() - 1
                    || tile_z >= self.max_z() - 1)
            {
                continue;
            }
            let level = i32::from(s.level);
            self.add_ground_loc(LocRequest {
                floor_level: level,
                scene_level: level,
                tile: [tile_x, tile_z],
                loc_id: s.id,
                angle,
                shape: sh,
                srt: s.srt,
            })?;
        }
        Ok(())
    }

    /// Place a ground loc.
    fn add_ground_loc(&mut self, request: LocRequest) -> anyhow::Result<()> {
        let LocRequest {
            floor_level,
            scene_level,
            tile: [tile_x, tile_z],
            loc_id,
            angle: loc_angle,
            shape: loc_shape,
            srt,
        } = request;
        if scene_level < self.min_level {
            self.min_level = scene_level;
        }
        let Some(loc_type) = self.locs.get(loc_id) else {
            return Ok(());
        };
        let loc_type = loc_type.clone();
        if self.prefs.textures == 0 && loc_type.istexture {
            return Ok(());
        }
        let floor_heights = self
            .heights(floor_level as usize)
            .ok_or_else(|| anyhow::anyhow!("level {floor_level} has no floor"))?;
        let Footprint {
            size: [size_x, size_z],
            centre: [world_x, centre_height, world_z],
            ..
        } = Footprint::new(
            &loc_type,
            [loc_shape, loc_angle],
            [tile_x, tile_z],
            floor_heights,
            [self.max_x(), self.max_z()],
        );
        let merged = self.is_blending && !self.underwater && loc_type.sharelight;
        if has_background_sound(self.locs, &loc_type) {
            self.sounds.push(crate::loc_sound::LocSoundSpawn {
                level: scene_level,
                x: tile_x,
                z: tile_z,
                angle: loc_angle,
                id: loc_id,
            });
        }
        // model_override == -1 && (!hasAnim || disableAnimLowDetail && animDetail == 0) && multiloc == null && !forceDynamic && !alwaysDynamic
        let is_static = (!loc_type.has_anim
            || (loc_type.disable_anim_low_detail && self.prefs.anim_detail == 0))
            && !loc_type.has_multiloc
            && !loc_type.force_dynamic
            && !loc_type.always_dynamic;
        if !is_static {
            self.stats.dynamic_placed += 1;
        }
        if let Some(s) = srt {
            self.stats.srt_locs += 1;
            if s.rot != [0.0, 0.0, 0.0, 1.0] || s.trans != [0.0; 3] || s.scale != [1.0; 3] {
                self.stats.srt_nonidentity += 1;
                if self.stats.srt_nonidentity <= 5 {
                    log::info!(
                        "[locs] loc {loc_id} shape {loc_shape} at {tile_x},{tile_z} srt {s:?}"
                    );
                }
            }
        }
        let spec = |shape: i32, angle: i32| EntitySpec {
            loc: &loc_type,
            level: scene_level,
            occlude_level: floor_level,
            at: [world_x, centre_height, world_z],
            shape,
            angle,
            merged,
            srt,
            is_static,
        };
        if shape::GROUND_DECOR == loc_shape {
            if self.prefs.ground_decoration != 0
                || loc_type.active_for(self.locs.allow_members.get()) != 0
                || loc_type.blockwalk == 1
                || loc_type.forcedecor
            {
                let e = self.new_ground_decor(spec(shape::GROUND_DECOR, loc_angle))?;
                self.scene.add_ground_decoration(
                    scene_level as usize,
                    tile_x as usize,
                    tile_z as usize,
                    e,
                );
                self.stats.placed += 1;
            }
        } else if shape::CENTREPIECE_STRAIGHT == loc_shape
            || shape::CENTREPIECE_DIAGONAL == loc_shape
        {
            let mut scenery = self.new_scenery(
                spec(loc_shape, loc_angle),
                [tile_x, tile_x + size_x - 1, tile_z, tile_z + size_z - 1],
                loc_type.layer != 2,
            )?;
            // Static: the model's horizontal radius / 4 (15 without a model);
            // dynamic: 15.
            let shade_value = if is_static {
                scenery
                    .model
                    .as_mut()
                    .map_or(15, |m| m.horizontal_radius() / 4)
            } else {
                15
            };
            if self.scene.add_entity(scenery) {
                self.stats.placed += 1;
                if loc_type.has_hard_shadow && self.is_blending {
                    let shade_value = shade_value.min(30);
                    let floor = self.floors[floor_level as usize].as_mut().expect("floor");
                    for shade_dx in 0..=size_x {
                        for shade_dz in 0..=size_z {
                            floor.set_level_shade_map(
                                tile_x + shade_dx,
                                tile_z + shade_dz,
                                shade_value,
                            );
                        }
                    }
                }
            }
        } else if shape::is_roof(loc_shape) || shape::is_roof_edge(loc_shape) {
            let roof_entity = self.new_scenery(
                spec(loc_shape, loc_angle),
                [tile_x, tile_x + size_x - 1, tile_z, tile_z + size_z - 1],
                true,
            )?;
            self.scene.add_entity(roof_entity);
            self.stats.placed += 1;
            if self.is_blending
                && !self.underwater
                && shape::is_roof(loc_shape)
                && shape::ROOF_DIAGONAL_WITH_ROOFEDGE != loc_shape
                && scene_level > 0
                && loc_type.occlude != 0
            {
                let stride = self.scene.max_z + 1;
                self.level_occludemap[scene_level as usize]
                    [tile_x as usize * stride + tile_z as usize] |= 0x4;
            }
        } else {
            let placement = Placement {
                loc_type: &loc_type,
                loc_shape,
                loc_angle,
                is_static,
                scene_level,
                floor_level,
                world: [world_x, centre_height, world_z],
                tile: [tile_x, tile_z],
                merged,
                size: [size_x, size_z],
                srt,
            };
            // A shape that is neither a wall nor a wall decoration places nothing.
            let _ = self.add_wall_loc(&placement)? || self.add_wall_decoration_loc(&placement)?;
        }
        Ok(())
    }

    fn occlude(&mut self, kind: i32, level: i32, x: i32, z: i32, height: i32, offset: i32) {
        self.scene.occlude_calls.push(OccludeMapCall {
            kind,
            level,
            x,
            z,
            height,
            offset,
        });
        if kind != 8 && kind != 16 {
            self.scene.set_occlude_marker(
                kind,
                level as usize,
                x as usize,
                z as usize,
                height,
                offset,
            );
        }
    }

    fn stamp(&mut self, level: i32, x: i32, z: i32) {
        if let Some(f) = self.floors[level as usize].as_mut() {
            f.set_level_shade_map(x, z, 50);
        }
    }

    /// Place a wall loc.
    fn add_wall_loc(&mut self, placement: &Placement<'_>) -> anyhow::Result<bool> {
        let Placement {
            loc_type,
            loc_shape,
            loc_angle,
            is_static,
            scene_level,
            floor_level,
            world: [world_x, world_y, world_z],
            tile: [tile_x, tile_z],
            merged,
            size: [size_x, size_z],
            srt,
        } = *placement;
        let spec = |shape: i32, angle: i32| EntitySpec {
            loc: loc_type,
            level: scene_level,
            occlude_level: floor_level,
            at: [world_x, world_y, world_z],
            shape,
            angle,
            merged,
            srt,
            is_static,
        };
        let shade = self.is_blending && loc_type.has_hard_shadow;
        if shape::WALL_STRAIGHT == loc_shape {
            let occludes = loc_type.occlude;
            let wall = self.new_wall(spec(loc_shape, loc_angle))?;
            self.scene.add_wall(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                wall,
                None,
            );
            self.stats.placed += 1;
            match loc_angle {
                0 => {
                    if shade {
                        self.stamp(floor_level, tile_x, tile_z);
                        self.stamp(floor_level, tile_x, tile_z + 1);
                    }
                    if occludes == 1 && !self.underwater {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                }
                1 => {
                    if shade {
                        self.stamp(floor_level, tile_x, tile_z + 1);
                        self.stamp(floor_level, tile_x + 1, tile_z + 1);
                    }
                    if occludes == 1 && !self.underwater {
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z + 1,
                            loc_type.occlude_width,
                            -loc_type.occlude_height,
                        );
                    }
                }
                2 => {
                    if shade {
                        self.stamp(floor_level, tile_x + 1, tile_z);
                        self.stamp(floor_level, tile_x + 1, tile_z + 1);
                    }
                    if occludes == 1 && !self.underwater {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x + 1,
                            tile_z,
                            loc_type.occlude_width,
                            -loc_type.occlude_height,
                        );
                    }
                }
                3 => {
                    if shade {
                        self.stamp(floor_level, tile_x, tile_z);
                        self.stamp(floor_level, tile_x + 1, tile_z);
                    }
                    if occludes == 1 && !self.underwater {
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                }
                _ => {}
            }
            if loc_type.walloff != 64 {
                self.scene.set_wall_decoration_offset(
                    scene_level as usize,
                    tile_x as usize,
                    tile_z as usize,
                    loc_type.walloff,
                );
            }
            Ok(true)
        } else if shape::WALL_DIAGONAL_CORNER == loc_shape {
            let wall = self.new_wall(spec(loc_shape, loc_angle))?;
            self.scene.add_wall(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                wall,
                None,
            );
            self.stats.placed += 1;
            if shade {
                match loc_angle {
                    0 => self.stamp(floor_level, tile_x, tile_z + 1),
                    1 => self.stamp(floor_level, tile_x + 1, tile_z + 1),
                    2 => self.stamp(floor_level, tile_x + 1, tile_z),
                    3 => self.stamp(floor_level, tile_x, tile_z),
                    _ => {}
                }
            }
            Ok(true)
        } else if shape::WALL_L == loc_shape {
            let corner_angle = (loc_angle + 1) & 0x3;
            let primary_wall = self.new_wall(spec(loc_shape, loc_angle + 4))?;
            let secondary_wall = self.new_wall(spec(loc_shape, corner_angle))?;
            self.scene.add_wall(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                primary_wall,
                Some(secondary_wall),
            );
            self.stats.placed += 1;
            if loc_type.occlude == 1 && !self.underwater {
                match loc_angle {
                    0 => {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z + 1,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                    1 => {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x + 1,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z + 1,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                    2 => {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x + 1,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                    3 => {
                        self.occlude(
                            1,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                        self.occlude(
                            2,
                            scene_level,
                            tile_x,
                            tile_z,
                            loc_type.occlude_width,
                            loc_type.occlude_height,
                        );
                    }
                    _ => {}
                }
            }
            if loc_type.walloff != 64 {
                self.scene.set_wall_decoration_offset(
                    scene_level as usize,
                    tile_x as usize,
                    tile_z as usize,
                    loc_type.walloff,
                );
            }
            Ok(true)
        } else if shape::WALL_SQUARE_CORNER == loc_shape {
            let wall = self.new_wall(spec(loc_shape, loc_angle))?;
            self.scene.add_wall(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                wall,
                None,
            );
            self.stats.placed += 1;
            if shade {
                match loc_angle {
                    0 => self.stamp(floor_level, tile_x, tile_z + 1),
                    1 => self.stamp(floor_level, tile_x + 1, tile_z + 1),
                    2 => self.stamp(floor_level, tile_x + 1, tile_z),
                    3 => self.stamp(floor_level, tile_x, tile_z),
                    _ => {}
                }
            }
            Ok(true)
        } else if shape::WALL_DIAGONAL == loc_shape {
            // Static footprint is the single tile; the dynamic one spans the
            // loc size.
            let (max_tx, max_tz) = if is_static {
                (tile_x, tile_z)
            } else {
                (tile_x + size_x - 1, tile_z + size_z - 1)
            };
            let diagonal_scenery = self.new_scenery(
                spec(loc_shape, loc_angle),
                [tile_x, max_tx, tile_z, max_tz],
                true,
            )?;
            self.scene.add_entity(diagonal_scenery);
            self.stats.placed += 1;
            if loc_type.occlude == 1 && !self.underwater {
                let occlude_kind = if (loc_angle & 0x1) == 0 { 8 } else { 16 };
                self.occlude(
                    occlude_kind,
                    scene_level,
                    tile_x,
                    tile_z,
                    loc_type.occlude_width,
                    0,
                );
            }
            if loc_type.walloff != 64 {
                self.scene.set_wall_decoration_offset(
                    scene_level as usize,
                    tile_x as usize,
                    tile_z as usize,
                    loc_type.walloff,
                );
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }

    /// Place a wall-decoration loc.
    fn add_wall_decoration_loc(&mut self, placement: &Placement<'_>) -> anyhow::Result<bool> {
        let Placement {
            loc_type,
            loc_shape,
            loc_angle,
            is_static,
            scene_level,
            floor_level,
            world: [world_x, world_y, world_z],
            tile: [tile_x, tile_z],
            srt,
            ..
        } = *placement;
        let spec = |shape: i32, angle: i32| EntitySpec {
            loc: loc_type,
            level: scene_level,
            occlude_level: floor_level,
            at: [world_x, world_y, world_z],
            shape,
            angle,
            merged: false,
            srt,
            is_static,
        };
        let wall_walloff = |s: &Self| -> Option<i32> {
            s.scene
                .get_wall(scene_level as usize, tile_x as usize, tile_z as usize)
                .and_then(|w| s.locs.get(w.loc_id))
                .map(|l| l.walloff)
        };
        if shape::WALLDECOR_STRAIGHT_NOOFFSET == loc_shape {
            let d = self.new_wall_decor(spec(loc_shape, loc_angle), [0, 0])?;
            self.scene.add_wall_decoration(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                d,
                None,
            );
            self.stats.placed += 1;
            Ok(true)
        } else if shape::WALLDECOR_STRAIGHT_OFFSET == loc_shape {
            let straight_offset = wall_walloff(self).map_or(65, |w| w + 1);
            let d = self.new_wall_decor(
                spec(loc_shape, loc_angle),
                [
                    ROTATION_FORWARD_X[loc_angle as usize] * straight_offset,
                    ROTATION_FORWARD_Z[loc_angle as usize] * straight_offset,
                ],
            )?;
            self.scene.add_wall_decoration(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                d,
                None,
            );
            self.stats.placed += 1;
            Ok(true)
        } else if shape::WALLDECOR_DIAGONAL_OFFSET == loc_shape {
            let diag_offset = wall_walloff(self).map_or(33, |w| w / 2 + 1);
            // Static path uses ROTATION_FORWARD_*, the dynamic path OFFSET_*.
            let (ox, oz) = if is_static {
                (
                    ROTATION_FORWARD_X[loc_angle as usize] * diag_offset,
                    ROTATION_FORWARD_Z[loc_angle as usize] * diag_offset,
                )
            } else {
                (
                    OFFSET_X[loc_angle as usize] * diag_offset,
                    OFFSET_Z[loc_angle as usize] * diag_offset,
                )
            };
            let d = self.new_wall_decor(spec(loc_shape, loc_angle + 4), [ox, oz])?;
            self.scene.add_wall_decoration(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                d,
                None,
            );
            self.stats.placed += 1;
            Ok(true)
        } else if shape::WALLDECOR_DIAGONAL_NOOFFSET == loc_shape {
            let back_angle = (loc_angle + 2) & 0x3;
            let d = self.new_wall_decor(spec(loc_shape, back_angle + 4), [0, 0])?;
            self.scene.add_wall_decoration(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                d,
                None,
            );
            self.stats.placed += 1;
            Ok(true)
        } else if shape::WALLDECOR_DIAGONAL_BOTH == loc_shape {
            let back_angle_both = (loc_angle + 2) & 0x3;
            let diag_both_offset = wall_walloff(self).map_or(33, |w| w / 2 + 1);
            let front_decor = self.new_wall_decor(
                spec(loc_shape, loc_angle + 4),
                [
                    OFFSET_X[loc_angle as usize] * diag_both_offset,
                    OFFSET_Z[loc_angle as usize] * diag_both_offset,
                ],
            )?;
            let back_decor = self.new_wall_decor(spec(loc_shape, back_angle_both + 4), [0, 0])?;
            self.scene.add_wall_decoration(
                scene_level as usize,
                tile_x as usize,
                tile_z as usize,
                front_decor,
                Some(back_decor),
            );
            self.stats.placed += 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    // -- entity constructors ------------------------------------------------

    /// Entity flags shared by the constructors: `2048 | 0x10000 (merged
    /// normals) | 0x80000 (antimacro)`.
    fn entity_flags(loc: &Loc, merged: bool) -> i32 {
        let mut f = 2048;
        if merged {
            f |= 0x10000;
        }
        if loc.antimacro {
            f |= 0x80000;
        }
        f
    }

    /// The entity transform's translation is `srt.trans + (x, y, z)` (all
    /// integer-valued floats, so exact in i32).
    fn placed(srt: Option<crate::map::LocSrt>, x: i32, y: i32, z: i32) -> (i32, i32, i32) {
        match srt {
            Some(s) => (
                x + s.trans[0] as i32,
                y + s.trans[1] as i32,
                z + s.trans[2] as i32,
            ),
            None => (x, y, z),
        }
    }

    /// Whether the loc casts a hard shadow: static entities also check the scenery-shadows
    /// preference, dynamic ones do not. The renderer always supports hard shadows.
    fn has_hard_shadow(&self, loc: &Loc, is_static: bool) -> bool {
        loc.hardshadow && !self.underwater && (!is_static || self.prefs.scenery_shadows != 0)
    }

    /// Model fetch: floors of `occludeLevel` and the level above, then the dynamic model.
    fn fetch_model(
        &mut self,
        loc: &Loc,
        flags: i32,
        occlude_level: i32,
        sh: i32,
        angle: i32,
        at: [i32; 3],
    ) -> anyhow::Result<Option<crate::gpumodel::GpuModel>> {
        let [x, y, z] = at;
        let floor = self.heights(occlude_level as usize);
        let above = if occlude_level < 3 {
            self.heights(occlude_level as usize + 1)
        } else {
            None
        };
        let request = loctype::ModelRequest {
            flags,
            shape: sh,
            rotation: angle,
        };
        let ground = loctype::LocGround { floor, above };
        match loctype::get_dynamic_model(self.models, loc, request, ground, [x, y, z]) {
            Ok(m) => Ok(m),
            Err(err) => {
                self.stats.model_failures += 1;
                log::warn!("[locs] loc {} shape {sh}: {err:#}", loc.id);
                Ok(None)
            }
        }
    }

    /// Shadow stamp for a freshly constructed static entity that casts a shadow: the dynamic
    /// model requested with the shadow flag (`0x40000`) has the same geometry as the entity's
    /// model plus its shadow, which is stamped at the transform's x/z into every floor up
    /// to `occludeLevel`. Every static construction site in ground, wall and wall-decoration
    /// placement is followed by this, so it lives here.
    fn stamp_entity_shadow(
        &mut self,
        model: &mut Option<crate::gpumodel::GpuModel>,
        has_hard_shadow: bool,
        occlude_level: i32,
        x: i32,
        z: i32,
    ) {
        if !has_hard_shadow {
            return;
        }
        let Some(m) = model.as_mut() else { return };
        if let Some(shadow) = m.hard_shadow(&self.sun) {
            crate::hardshadow::apply_shadow(
                self.floors,
                &shadow,
                occlude_level as usize,
                x,
                z,
                &self.sun,
                None,
            );
            self.stats.entity_shadows += 1;
        }
    }

    fn apply_tint(&self, loc: &Loc, model: &mut Option<crate::gpumodel::GpuModel>) {
        if loc.antimacro {
            if let Some(m) = model.as_mut() {
                let (h, s, l, w) = self.loc_tint;
                m.tint(h, s, l, w);
            }
        }
    }

    /// A static scenery entity (or a model-less placeholder for a dynamic
    /// loc) over the footprint `[min x, max x, min z, max z]` in tiles.
    fn new_scenery(
        &mut self,
        spec: EntitySpec<'_>,
        footprint: [i32; 4],
        primary_layer: bool,
    ) -> anyhow::Result<SceneryEntity> {
        let EntitySpec {
            loc,
            level,
            occlude_level,
            at: [x, y, z],
            shape: sh,
            angle,
            merged,
            srt,
            is_static,
        } = spec;
        let [min_tx, max_tx, min_tz, max_tz] = footprint;
        let diag: i8 = if shape::WALL_DIAGONAL == sh {
            if (angle & 0x1) == 0 {
                1
            } else {
                2
            }
        } else {
            0
        };
        let merged = merged && is_static;
        // The model fetch takes the transform's x/z and the constructor's y.
        let (tx, ty, tz) = Self::placed(srt, x, y, z);
        let mut model = None;
        if is_static {
            let flags = Self::entity_flags(loc, merged);
            let (msh, mang) = if shape::CENTREPIECE_DIAGONAL == sh {
                (shape::CENTREPIECE_STRAIGHT, angle + 4)
            } else {
                (sh, angle)
            };
            model = self.fetch_model(loc, flags, occlude_level, msh, mang, [tx, y, tz])?;
            self.apply_tint(loc, &mut model);
            let hs = self.has_hard_shadow(loc, true);
            self.stamp_entity_shadow(&mut model, hs, occlude_level, tx, tz);
        }
        Ok(SceneryEntity {
            level,
            occlude_level,
            x: tx,
            y: ty,
            z: tz,
            min_tx,
            max_tx,
            min_tz,
            max_tz,
            raised: loc.raiseobject == 1,
            diag,
            model_y: y,
            loc_id: loc.id,
            shape: sh,
            angle,
            active: loc.active_for(self.locs.allow_members.get()) != 0 && !self.underwater,
            use_merged_normals: merged,
            has_hard_shadow: self.has_hard_shadow(loc, is_static),
            primary_layer,
            srt,
            dynamic: !is_static,
            model,
        })
    }

    /// A static wall entity or its dynamic placeholder.
    fn new_wall(&mut self, spec: EntitySpec<'_>) -> anyhow::Result<WallEntity> {
        let EntitySpec {
            loc,
            level,
            occlude_level,
            at: [x, y, z],
            shape: sh,
            angle,
            merged,
            srt,
            is_static,
        } = spec;
        let wall_type = if shape::WALL_DIAGONAL_CORNER == sh || shape::WALL_SQUARE_CORNER == sh {
            WALL_CORNER_TYPE[(angle & 0x3) as usize]
        } else {
            WALL_TYPE[(angle & 0x3) as usize]
        };
        let merged = merged && is_static;
        let (tx, ty, tz) = Self::placed(srt, x, y, z);
        let mut model = None;
        if is_static {
            let flags = Self::entity_flags(loc, merged);
            model = self.fetch_model(loc, flags, occlude_level, sh, angle, [tx, y, tz])?;
            self.apply_tint(loc, &mut model);
            let hs = self.has_hard_shadow(loc, true);
            self.stamp_entity_shadow(&mut model, hs, occlude_level, tx, tz);
        }
        Ok(WallEntity {
            level,
            occlude_level,
            x: tx,
            y: ty,
            z: tz,
            wall_type,
            loc_id: loc.id,
            shape: sh,
            angle,
            active: loc.active_for(self.locs.allow_members.get()) != 0 && !self.underwater,
            use_merged_normals: merged,
            has_hard_shadow: self.has_hard_shadow(loc, is_static),
            srt,
            dynamic: !is_static,
            model,
        })
    }

    /// A static wall decoration entity (never merged) or its dynamic
    /// placeholder, pushed out from the wall by `offset`.
    fn new_wall_decor(
        &mut self,
        spec: EntitySpec<'_>,
        offset: [i32; 2],
    ) -> anyhow::Result<WallDecorEntity> {
        let EntitySpec {
            loc,
            level,
            occlude_level,
            at: [x, y, z],
            shape: sh,
            angle,
            srt,
            is_static,
            ..
        } = spec;
        let [offset_x, offset_z] = offset;
        let (tx, ty, tz) = Self::placed(srt, x, y, z);
        let mut model = None;
        if is_static {
            let flags = Self::entity_flags(loc, false);
            model = self.fetch_model(loc, flags, occlude_level, sh, angle, [tx, y, tz])?;
            self.apply_tint(loc, &mut model);
            let hs = self.has_hard_shadow(loc, true);
            self.stamp_entity_shadow(&mut model, hs, occlude_level, tx, tz);
        }
        Ok(WallDecorEntity {
            level,
            occlude_level,
            x: tx,
            y: ty,
            z: tz,
            offset_x: (offset_x as i16) as i32,
            offset_z: (offset_z as i16) as i32,
            loc_id: loc.id,
            shape: sh,
            angle,
            active: loc.active_for(self.locs.allow_members.get()) != 0 && !self.underwater,
            has_hard_shadow: self.has_hard_shadow(loc, is_static),
            srt,
            dynamic: !is_static,
            model,
        })
    }

    /// A static ground decoration entity or its dynamic placeholder. The
    /// spec's shape is ignored: a ground decoration always has its own.
    fn new_ground_decor(&mut self, spec: EntitySpec<'_>) -> anyhow::Result<GroundDecorEntity> {
        let EntitySpec {
            loc,
            level,
            occlude_level,
            at: [x, y, z],
            angle,
            merged,
            srt,
            is_static,
            ..
        } = spec;
        let merged = merged && is_static;
        let (tx, ty, tz) = Self::placed(srt, x, y, z);
        let mut model = None;
        if is_static {
            let flags = Self::entity_flags(loc, merged);
            model = self.fetch_model(
                loc,
                flags,
                occlude_level,
                shape::GROUND_DECOR,
                angle,
                [tx, y, tz],
            )?;
            self.apply_tint(loc, &mut model);
            let hs = self.has_hard_shadow(loc, true);
            self.stamp_entity_shadow(&mut model, hs, occlude_level, tx, tz);
        }
        Ok(GroundDecorEntity {
            level,
            occlude_level,
            x: tx,
            y: ty,
            z: tz,
            decor_height: (loc.decor_height as i16) as i32,
            loc_id: loc.id,
            angle,
            active: loc.active_for(self.locs.allow_members.get()) != 0 && !self.underwater,
            use_merged_normals: merged,
            has_hard_shadow: self.has_hard_shadow(loc, is_static),
            srt,
            dynamic: !is_static,
            model,
        })
    }

    // -- build --------------------------------------------------------------

    /// Build the normal scene: model building,
    /// bridges from tile flags, greedy roof occluder rectangles.
    pub fn build(&mut self, flags: &SceneLevelTileFlags, heights: &[FloorHeights]) {
        rs910_core::profile::scope!("build models");
        self.scene.build_models(heights);
        if self.levels > 1 {
            for bridge_x in 0..self.scene.max_x {
                for bridge_z in 0..self.scene.max_z {
                    if (flags.get(1, bridge_x, bridge_z) & 0x2) == 2 {
                        self.scene.set_bridge(bridge_x, bridge_z);
                    }
                }
            }
        }
        let stride = self.scene.max_z + 1;
        let max_x = self.max_x();
        let max_z = self.max_z();
        for level_index in 0..self.levels {
            for seed_z in 0..=max_z {
                for seed_x in 0..=max_x {
                    let occ = |m: &[Vec<u8>], x: i32, z: i32| {
                        m[level_index][x as usize * stride + z as usize] & 0x4
                    };
                    if occ(self.level_occludemap, seed_x, seed_z) == 0 {
                        continue;
                    }
                    let mut rect_min_x = seed_x;
                    let mut rect_max_x = seed_x;
                    let mut rect_min_z = seed_z;
                    let mut rect_max_z = seed_z;
                    while rect_min_z > 0
                        && occ(self.level_occludemap, seed_x, rect_min_z - 1) != 0
                        && rect_max_z - rect_min_z < 10
                    {
                        rect_min_z -= 1;
                    }
                    while rect_max_z < max_z
                        && occ(self.level_occludemap, seed_x, rect_max_z + 1) != 0
                        && rect_max_z - rect_min_z < 10
                    {
                        rect_max_z += 1;
                    }
                    'grow_west: while rect_min_x > 0 && rect_max_x - rect_min_x < 10 {
                        for west_scan_z in rect_min_z..=rect_max_z {
                            if occ(self.level_occludemap, rect_min_x - 1, west_scan_z) == 0 {
                                break 'grow_west;
                            }
                        }
                        rect_min_x -= 1;
                    }
                    'grow_east: while rect_max_x < max_x && rect_max_x - rect_min_x < 10 {
                        for east_scan_z in rect_min_z..=rect_max_z {
                            if occ(self.level_occludemap, rect_max_x + 1, east_scan_z) == 0 {
                                break 'grow_east;
                            }
                        }
                        rect_max_x += 1;
                    }
                    if (rect_max_x - rect_min_x + 1) * (rect_max_z - rect_min_z + 1) >= 4 {
                        let rect_height = heights[level_index]
                            .get_tile_height(rect_min_x as usize, rect_min_z as usize);
                        self.scene.occluders.push(OccluderRect {
                            kind: 4,
                            level: level_index as i32,
                            x0: rect_min_x << 9,
                            x1: (rect_max_x << 9) + 512,
                            z0: rect_min_z << 9,
                            z1: (rect_max_z << 9) + 512,
                            y0: rect_height,
                            y1: rect_height,
                        });
                        for clear_x in rect_min_x..=rect_max_x {
                            for clear_z in rect_min_z..=rect_max_z {
                                self.level_occludemap[level_index]
                                    [clear_x as usize * stride + clear_z as usize] &= !0x4;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `(kind, x, z)` of each occluder clear made when
/// the loc `loc` (`shape`/`angle` of the removed entity) leaves `layer` at
/// scene tile `(x, z)`. Only a layer-0 loc with `occlude == 1` clears its
/// wall marker (the first one of a `WALL_L`), and a layer-2
/// `WALL_DIAGONAL` drops its diagonal occluder whatever its `occlude`.
#[must_use]
pub fn remove_loc_occluders(
    loc: &Loc,
    layer: i32,
    shape: i32,
    angle: i32,
    x: i32,
    z: i32,
) -> Vec<(i32, i32, i32)> {
    match layer {
        0 if loc.occlude == 1 => match angle {
            0 => vec![(1, x, z)],
            1 => vec![(2, x, z + 1)],
            2 => vec![(1, x + 1, z)],
            3 => vec![(2, x, z)],
            _ => vec![],
        },
        2 if shape::WALL_DIAGONAL == shape => {
            vec![(if (angle & 0x1) == 0 { 8 } else { 16 }, x, z)]
        }
        _ => vec![],
    }
}

/// The level-occlusion marker calls a loc added at scene tile `(x, z)` makes (wall
/// placement for `WALL_STRAIGHT`, `WALL_L` and `WALL_DIAGONAL`; the normal, non-underwater
/// scene), as [`OccludeMapCall`]s with `level`. Mirrors the placement in [`LocPlacer`] for a
/// loc that a runtime loc change adds.
#[must_use]
pub fn add_loc_occluders(
    loc: &Loc,
    shape: i32,
    angle: i32,
    level: i32,
    x: i32,
    z: i32,
) -> Vec<OccludeMapCall> {
    let (w, h) = (loc.occlude_width, loc.occlude_height);
    let call = |kind, x, z, height, offset| OccludeMapCall {
        kind,
        level,
        x,
        z,
        height,
        offset,
    };
    if loc.occlude != 1 {
        return vec![];
    }
    if shape::WALL_STRAIGHT == shape {
        match angle {
            0 => vec![call(1, x, z, w, h)],
            1 => vec![call(2, x, z + 1, w, -h)],
            2 => vec![call(1, x + 1, z, w, -h)],
            3 => vec![call(2, x, z, w, h)],
            _ => vec![],
        }
    } else if shape::WALL_L == shape {
        match angle {
            0 => vec![call(1, x, z, w, h), call(2, x, z + 1, w, h)],
            1 => vec![call(1, x + 1, z, w, h), call(2, x, z + 1, w, h)],
            2 => vec![call(1, x + 1, z, w, h), call(2, x, z, w, h)],
            3 => vec![call(1, x, z, w, h), call(2, x, z, w, h)],
            _ => vec![],
        }
    } else if shape::WALL_DIAGONAL == shape {
        vec![call(if (angle & 0x1) == 0 { 8 } else { 16 }, x, z, w, 0)]
    } else {
        vec![]
    }
}

/// Scene entities per location key, plus the dynamic wall / decoration
/// removed with them.
pub type LocationRefs = std::collections::HashMap<
    (i32, i32, i32, i32),
    (crate::scene::EntityRef, Option<crate::scene::EntityRef>),
>;

/// The located entity of every occupied slot of the scene (wall, wall decoration, ground
/// decoration and the primary-layer loc entity), keyed like a loc change request
/// `(level, layer, x, z)`. The same traversal as the location snapshot
/// (`game_runtime::capture_scene`).
/// Each value is the located entity plus, for layers 0/1, the tile's
/// second wall / wall decoration that a loc removal drops with it.
#[must_use]
pub fn location_refs(scene: &Scene) -> LocationRefs {
    use crate::scene::{EntityRef, PrimaryRef};
    let mut out = std::collections::HashMap::new();
    for level in 0..scene.max_level {
        for x in 0..scene.max_x {
            for z in 0..scene.max_z {
                let Some(tile) = scene.tile(level, x, z) else {
                    continue;
                };
                let key = |layer| (level as i32, layer, x as i32, z as i32);
                if let Some(i) = tile.wall {
                    out.insert(
                        key(0),
                        (EntityRef::Wall(i), tile.dynamic_wall.map(EntityRef::Wall)),
                    );
                }
                if let Some(i) = tile.wall_decoration {
                    out.insert(
                        key(1),
                        (
                            EntityRef::WallDecor(i),
                            tile.dynamic_wall_decoration.map(EntityRef::WallDecor),
                        ),
                    );
                }
                if let Some(i) = tile.entities.iter().find_map(|&r| match r {
                    PrimaryRef::Scenery(i)
                        if scene.scenery[i].primary_layer
                            && scene.scenery[i].min_tx == x as i32
                            && scene.scenery[i].min_tz == z as i32 =>
                    {
                        Some(i)
                    }
                    _ => None,
                }) {
                    out.insert(key(2), (EntityRef::Scenery(i), None));
                }
                if let Some(i) = tile.ground_decoration {
                    out.insert(key(3), (EntityRef::GroundDecor(i), None));
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod loc_change_tests {
    use super::*;

    fn wall(occlude: i32) -> Loc {
        let mut loc = crate::config::decode_loc(0, &[0]).expect("empty loc");
        loc.occlude = occlude;
        loc.occlude_width = 7;
        loc.occlude_height = 3;
        loc
    }

    /// `removeLoc` clears exactly the marker `addWallLoc` set for a straight
    /// wall of every angle (kind, tile), and nothing for `occlude != 1`.
    #[test]
    fn remove_matches_add_for_straight_walls() {
        let loc = wall(1);
        for angle in 0..4 {
            let added = add_loc_occluders(&loc, shape::WALL_STRAIGHT, angle, 1, 10, 20);
            let removed = remove_loc_occluders(&loc, 0, shape::WALL_STRAIGHT, angle, 10, 20);
            assert_eq!(
                added.iter().map(|c| (c.kind, c.x, c.z)).collect::<Vec<_>>(),
                removed
            );
        }
        assert!(remove_loc_occluders(&wall(0), 0, shape::WALL_STRAIGHT, 0, 1, 1).is_empty());
        assert!(add_loc_occluders(&wall(0), shape::WALL_STRAIGHT, 0, 0, 1, 1).is_empty());
    }

    /// Diagonal walls use kind 8 for even and 16 for odd angles; removal
    /// does not test `occlude`.
    #[test]
    fn diagonal_kinds() {
        assert_eq!(
            remove_loc_occluders(&wall(0), 2, shape::WALL_DIAGONAL, 1, 4, 5),
            vec![(16, 4, 5)]
        );
        let add = add_loc_occluders(&wall(1), shape::WALL_DIAGONAL, 2, 0, 4, 5);
        assert_eq!((add[0].kind, add[0].height, add[0].offset), (8, 7, 0));
        // A WALL_L sets two markers but removal clears only the angle's first.
        assert_eq!(
            add_loc_occluders(&wall(1), shape::WALL_L, 1, 0, 4, 5).len(),
            2
        );
        assert_eq!(
            remove_loc_occluders(&wall(1), 0, shape::WALL_L, 1, 4, 5),
            vec![(2, 4, 6)]
        );
    }
}
