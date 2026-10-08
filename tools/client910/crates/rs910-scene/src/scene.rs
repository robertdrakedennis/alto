//! The static part of the scene: tile grid, bridges, the four static loc entity kinds
//! (scenery, wall, wall decoration, ground decoration), their placement, wall-decoration
//! offsets, model building and normal merging between neighbouring locs.
//!
//! Entities live in arenas; tiles hold ids. The draw buckets (opaque, transparent, pending)
//! are kept in push order; the draw pass reads them reversed, because the original lists
//! prepend.
//!
//! Dynamic models are created at draw time by `dynamic_scene`. Not here: obj stacks, lights,
//! water-fog propagation for the underwater set (needs the underwater scene), hard-shadow
//! stamps and the draw path.

use crate::floor::FloorHeights;
use crate::gpumodel::GpuModel;
use crate::map::LocSrt;

/// Which arena an entity lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EntityRef {
    /// A static scenery entity in [`Scene::scenery`].
    Scenery(usize),
    /// A static wall entity in [`Scene::walls`].
    Wall(usize),
    /// A static wall decoration in [`Scene::wall_decors`].
    WallDecor(usize),
    /// A static ground decoration in [`Scene::ground_decors`].
    GroundDecor(usize),
    /// Per-frame primary-layer entity in the tiles' entity lists, separate from the loc arenas.
    Temporary(usize),
}

/// The rotation/scale part of a static loc's transform that its draw matrix applies:
/// `None` for a loc without one or whose transform is translation only (the translation is
/// already in the entity position).
pub fn entity_srt(scene: &Scene, e: EntityRef) -> Option<LocSrt> {
    let srt = match e {
        EntityRef::Scenery(i) => scene.scenery.get(i)?.srt,
        EntityRef::Wall(i) => scene.walls.get(i)?.srt,
        EntityRef::WallDecor(i) => scene.wall_decors.get(i)?.srt,
        EntityRef::GroundDecor(i) => scene.ground_decors.get(i)?.srt,
        EntityRef::Temporary(_) => None,
    }?;
    (srt.rot != [0.0, 0.0, 0.0, 1.0] || srt.scale != [1.0; 3]).then_some(srt)
}

/// The model as drawn through its entity matrix's rotation and scale: a copy with
/// [`GpuModel::apply_srt`] for a rotated/scaled loc, else the model itself.
/// The GPU meshes take a world translation only, so the linear part is
/// applied to the vertices here.
pub fn srt_model(model: &GpuModel, srt: Option<LocSrt>) -> std::borrow::Cow<'_, GpuModel> {
    match srt {
        Some(s) => {
            let mut m = model.clone();
            m.apply_srt([
                s.rot[0], s.rot[1], s.rot[2], s.rot[3], 0.0, 0.0, 0.0, s.scale[0], s.scale[1],
                s.scale[2],
            ]);
            std::borrow::Cow::Owned(m)
        }
        None => std::borrow::Cow::Borrowed(model),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PrimaryRef {
    Scenery(usize),
    Temporary(usize),
}
#[derive(Clone, Debug)]
pub struct TemporaryEntity {
    pub player: usize,
    /// NPC slot for transient world bodies; `None` covers players and effects.
    pub npc_index: Option<usize>,
    /// Retained zone location key when this temporary is a loc replacement.
    /// Other temporary owners leave this unset so the replacement can be
    /// promoted into the persistent scene layer without guessing by tile.
    pub location_key: Option<(i32, i32, i32, i32)>,
    /// Menu identity for a pickable transient that is neither a player nor
    /// an NPC (a loc or an obj stack in the scene's menu options).
    pub pick: Option<TemporaryPick>,
    pub transient: bool,
    pub level: i32,
    pub occlude_level: i32,
    pub position: [f32; 3],
    pub bounds: [i32; 4],
    pub overlay_height: i32,
    pub transparent: bool,
    /// A ground shadow drawn beneath its owner's body: translucent, written
    /// without depth so the body drawn after it stays whole.
    pub spot_shadow: bool,
    pub model: Option<GpuModel>,
}

/// What a pickable transient stands for when the scene builds its menu options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TemporaryPick {
    /// A changed/added loc (a dynamic entity): its id, shape, angle and scene tile
    /// (the minimum tile for primary locs).
    Loc {
        id: i32,
        shape: i32,
        angle: i32,
        tile: [i32; 2],
        /// The request's transform when its model carries it baked in.
        srt: Option<LocSrtPick>,
    },
    /// One model of the obj stack on this scene tile: `rank` 0, 1, 2
    /// is its primary, secondary or tertiary object.
    Obj { tile: [i32; 2], rank: usize },
}

/// A changed loc's transform (baked into its model by `GpuModel::apply_srt`) with the
/// model's bounds before baking: the untransformed model is drawn and picked through the
/// transform matrix, the same [`entity_srt`] matrix static locs use
/// (`player_picking::entity_mvp`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocSrtPick {
    pub srt: LocSrt,
    /// `[minX, minY, minZ, maxX, maxY, maxZ]` of the untransformed model.
    pub bounds: [i32; 6],
    pub horizontal_radius: i32,
}
/// `TemporaryPick` compares picks by identity; the decoded SRT floats are
/// finite `g2s` ratios, never NaN.
impl Eq for LocSrtPick {}

/// A static scenery loc: a primary-layer entity with its own position and model.
#[derive(Clone, Debug)]
pub struct SceneryEntity {
    /// Render level.
    pub level: i32,
    /// Level used for occlusion.
    pub occlude_level: i32,
    /// Entity translation (integer-valued).
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Footprint in scene tiles (min/max x and z).
    pub min_tx: i32,
    pub max_tx: i32,
    pub min_tz: i32,
    pub max_tz: i32,
    /// Whether the entity is raised.
    pub raised: bool,
    /// Diagonal-wall hint 0/1/2.
    pub diag: i8,
    /// The y handed to the dynamic model.
    #[allow(
        dead_code,
        reason = "the y handed to the dynamic model; nothing reads it yet"
    )]
    pub model_y: i32,
    /// `locId`.
    pub loc_id: u32,
    /// `shape`.
    pub shape: i32,
    /// `angle`.
    pub angle: i32,
    /// `active`.
    pub active: bool,
    /// `useMergedNormals` (cleared by `applyLighting`).
    pub use_merged_normals: bool,
    /// `hasHardShadow`.
    pub has_hard_shadow: bool,
    /// Whether the loc sits on the primary layer.
    pub primary_layer: bool,
    /// The loc's rotation/scale transform, when it carried one.
    pub srt: Option<LocSrt>,
    /// A dynamic scenery entity: no initial model; `dynamic_scene` supplies
    /// its pose at draw time while `isModelReady` keeps it in the pending list.
    pub dynamic: bool,
    /// `model`.
    pub model: Option<GpuModel>,
}

/// A static wall entity.
#[derive(Clone, Debug)]
pub struct WallEntity {
    pub level: i32,
    pub occlude_level: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Wall type (1/2/4/8 sides, 16/32/64/128 corners).
    pub wall_type: i32,
    pub loc_id: u32,
    pub shape: i32,
    pub angle: i32,
    pub active: bool,
    /// Whether normals are merged with neighbours (cleared once lighting is applied).
    pub use_merged_normals: bool,
    pub has_hard_shadow: bool,
    pub srt: Option<LocSrt>,
    /// Dynamic wall placeholder (see [`SceneryEntity::dynamic`]).
    pub dynamic: bool,
    pub model: Option<GpuModel>,
}

/// A static wall decoration.
#[derive(Clone, Debug)]
pub struct WallDecorEntity {
    pub level: i32,
    pub occlude_level: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// X offset, scaled by [`Scene::set_wall_decoration_offset`].
    pub offset_x: i32,
    /// Z offset.
    pub offset_z: i32,
    pub loc_id: u32,
    pub shape: i32,
    pub angle: i32,
    pub active: bool,
    pub has_hard_shadow: bool,
    pub srt: Option<LocSrt>,
    /// Dynamic wall-decoration placeholder (see [`SceneryEntity::dynamic`]).
    pub dynamic: bool,
    pub model: Option<GpuModel>,
}

/// A static ground decoration.
#[derive(Clone, Debug)]
pub struct GroundDecorEntity {
    pub level: i32,
    pub occlude_level: i32,
    pub x: i32,
    pub y: i32,
    pub z: i32,
    /// Decoration height.
    pub decor_height: i32,
    pub loc_id: u32,
    pub angle: i32,
    pub active: bool,
    /// Whether normals are merged with neighbours.
    pub use_merged_normals: bool,
    pub has_hard_shadow: bool,
    pub srt: Option<LocSrt>,
    /// Dynamic ground-decoration placeholder (see [`SceneryEntity::dynamic`]).
    pub dynamic: bool,
    pub model: Option<GpuModel>,
}

/// `Tile`.
#[derive(Clone, Debug, Default)]
pub struct Tile {
    /// `level` (render level; diverges from the plane index on bridges).
    pub level: i8,
    /// `bridge`: the displaced plane-0 tile.
    pub bridge: Option<Box<Tile>>,
    /// `wall`.
    pub wall: Option<usize>,
    /// `dynamicWall` (second wall of `WALL_L`).
    pub dynamic_wall: Option<usize>,
    /// `wallDecoration`.
    pub wall_decoration: Option<usize>,
    /// `dynamicWallDecoration`.
    pub dynamic_wall_decoration: Option<usize>,
    /// `groundDecoration`.
    pub ground_decoration: Option<usize>,
    /// Primary-layer entities on this tile, appended at the tail.
    pub entities: Vec<PrimaryRef>,
    /// X-plane occluder height / offset.
    pub occlude_x: (i16, i16),
    /// Z-plane occluder height / offset.
    pub occlude_z: (i16, i16),
}

impl Tile {
    fn new(level: i32) -> Self {
        Self {
            level: level as i8,
            ..Self::default()
        }
    }
}

/// A level-occlusion marker (kind, level, tile, height, offset) as recorded during placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OccludeMapCall {
    pub kind: i32,
    pub level: i32,
    pub x: i32,
    pub z: i32,
    pub height: i32,
    pub offset: i32,
}

/// An occluder rectangle (kind, level, x/z/y extents) as recorded during placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OccluderRect {
    pub kind: i32,
    pub level: i32,
    pub x0: i32,
    pub x1: i32,
    pub z0: i32,
    pub z1: i32,
    pub y0: i32,
    pub y1: i32,
}

/// `Scene` (static slice).
#[derive(Clone, Debug)]
pub struct Scene {
    render_identity: std::sync::Arc<()>,
    /// `size` (9).
    pub size: i32,
    /// `tileSize` (512).
    pub tile_size: i32,
    /// `halfTileSize` (256).
    pub half_tile_size: i32,
    /// `maxLevel`.
    pub max_level: usize,
    /// `maxTileX`.
    pub max_x: usize,
    /// `maxTileZ`.
    pub max_z: usize,
    /// `normalTiles[level][x][z]`.
    tiles: Vec<Option<Tile>>,
    pub scenery: Vec<SceneryEntity>,
    pub walls: Vec<WallEntity>,
    pub wall_decors: Vec<WallDecorEntity>,
    pub ground_decors: Vec<GroundDecorEntity>,
    pub temporary: Vec<TemporaryEntity>,
    /// Opaque draw bucket in push order (the original list prepends).
    pub opaque: Vec<EntityRef>,
    /// Transparent draw bucket in push order.
    pub transparent: Vec<EntityRef>,
    /// Pending draw bucket in push order.
    pub pending: Vec<EntityRef>,
    /// Recorded `setLevelOccludeMap` calls (phase E consumes them).
    pub occlude_calls: Vec<OccludeMapCall>,
    /// Recorded `addOccluder` calls.
    pub occluders: Vec<OccluderRect>,
}

impl Scene {
    /// Retained construction identity. Replacing a scene creates a new token
    /// even when its struct or geometry allocation reuses the same address.
    pub fn render_identity(&self) -> &std::sync::Arc<()> {
        &self.render_identity
    }

    /// A read-only render projection. Model storage travels separately by resource identity.
    pub fn render_metadata(&self) -> Self {
        Self {
            render_identity: self.render_identity.clone(),
            size: self.size,
            tile_size: self.tile_size,
            half_tile_size: self.half_tile_size,
            max_level: self.max_level,
            max_x: self.max_x,
            max_z: self.max_z,
            tiles: self.tiles.clone(),
            opaque: self.opaque.clone(),
            transparent: self.transparent.clone(),
            pending: self.pending.clone(),
            occlude_calls: self.occlude_calls.clone(),
            occluders: self.occluders.clone(),
            scenery: self
                .scenery
                .iter()
                .map(SceneryEntity::render_metadata)
                .collect(),
            walls: self.walls.iter().map(WallEntity::render_metadata).collect(),
            wall_decors: self
                .wall_decors
                .iter()
                .map(WallDecorEntity::render_metadata)
                .collect(),
            ground_decors: self
                .ground_decors
                .iter()
                .map(GroundDecorEntity::render_metadata)
                .collect(),
            temporary: self
                .temporary
                .iter()
                .map(TemporaryEntity::render_metadata)
                .collect(),
        }
    }

    /// Adds a temporary entity to the normal scene. Tile lists append
    /// in insertion order and survive as empty tiles after clearEntities.
    pub fn add_temporary(&mut self, entity: TemporaryEntity) -> usize {
        let id = self.temporary.len();
        let b = entity.bounds;
        for x in b[0].clamp(0, self.max_x as i32 - 1)..=b[1].clamp(0, self.max_x as i32 - 1) {
            for z in b[2].clamp(0, self.max_z as i32 - 1)..=b[3].clamp(0, self.max_z as i32 - 1) {
                if let Some(tile) = self.get_tile_coerce(entity.level as usize, x, z) {
                    tile.entities.push(PrimaryRef::Temporary(id));
                }
            }
        }
        self.temporary.push(entity);
        id
    }
    /// Removes every temporary entity from its footprint tiles and clears the arena.
    pub fn clear_temporary(&mut self) {
        for id in 0..self.temporary.len() {
            let level = self.temporary[id].level as usize;
            let b = self.temporary[id].bounds;
            for x in b[0].clamp(0, self.max_x as i32 - 1)..=b[1].clamp(0, self.max_x as i32 - 1) {
                for z in b[2].clamp(0, self.max_z as i32 - 1)..=b[3].clamp(0, self.max_z as i32 - 1)
                {
                    if let Some(t) = self.tile_mut(level, x as usize, z as usize) {
                        if let Some(at) = t
                            .entities
                            .iter()
                            .position(|r| *r == PrimaryRef::Temporary(id))
                        {
                            t.entities.remove(at);
                        }
                    }
                }
            }
        }
        self.temporary.clear();
    }
    pub fn primary_bounds(&self, source: PrimaryRef) -> [i32; 4] {
        match source {
            PrimaryRef::Scenery(i) => {
                let e = &self.scenery[i];
                [e.min_tx, e.max_tx, e.min_tz, e.max_tz]
            }
            PrimaryRef::Temporary(i) => self.temporary[i].bounds,
        }
    }
    /// Creates an empty static scene of the given tile-size exponent, level count and extent.
    #[must_use]
    pub fn new(size: i32, levels: usize, max_x: usize, max_z: usize) -> Self {
        Self {
            render_identity: std::sync::Arc::new(()),
            size,
            tile_size: 1 << size,
            half_tile_size: (1 << size) >> 1,
            max_level: levels,
            max_x,
            max_z,
            tiles: vec![None; levels * max_x * max_z],
            scenery: Vec::new(),
            walls: Vec::new(),
            wall_decors: Vec::new(),
            ground_decors: Vec::new(),
            temporary: Vec::new(),
            opaque: Vec::new(),
            transparent: Vec::new(),
            pending: Vec::new(),
            occlude_calls: Vec::new(),
            occluders: Vec::new(),
        }
    }

    fn index(&self, level: usize, x: usize, z: usize) -> usize {
        (level * self.max_x + x) * self.max_z + z
    }

    /// `levelTiles[level][x][z]`.
    #[must_use]
    pub fn tile(&self, level: usize, x: usize, z: usize) -> Option<&Tile> {
        self.tiles[self.index(level, x, z)].as_ref()
    }

    fn tile_mut(&mut self, level: usize, x: usize, z: usize) -> Option<&mut Tile> {
        let i = self.index(level, x, z);
        self.tiles[i].as_mut()
    }

    /// Level-occlusion marker: create only this
    /// tile, then store the signed-short wall height and offset. Bridges
    /// subsequently move these markers with the tile.
    pub fn set_occlude_marker(
        &mut self,
        kind: i32,
        level: usize,
        x: usize,
        z: usize,
        height: i32,
        offset: i32,
    ) {
        let i = self.index(level, x, z);
        let tile = self.tiles[i].get_or_insert_with(|| Tile::new(level as i32));
        if kind == 1 {
            tile.occlude_x = (height as i16, offset as i16);
        } else if kind == 2 {
            tile.occlude_z = (height as i16, offset as i16);
        }
    }

    fn bridged_column(&self, x: usize, z: usize) -> bool {
        self.tile(0, x, z).is_some_and(|t| t.bridge.is_some())
    }

    /// Allocate the tile at `(level, x, z)` and every plane below it.
    pub fn create_tile(&mut self, level: usize, x: usize, z: usize) {
        let is_bridged = self.bridged_column(x, z);
        for tile_level in (0..=level).rev() {
            let i = self.index(tile_level, x, z);
            if self.tiles[i].is_none() {
                let mut t = Tile::new(tile_level as i32);
                if is_bridged {
                    t.level += 1;
                }
                self.tiles[i] = Some(t);
            }
        }
    }

    /// The tile at `(level, x, z)`, created on demand unless the column is
    /// bridged and `level` is the top plane.
    pub fn get_tile(&mut self, level: usize, x: usize, z: usize) -> Option<&mut Tile> {
        if self.tile(level, x, z).is_none() {
            let is_bridged = self.bridged_column(x, z);
            if is_bridged && level >= self.max_level - 1 {
                return None;
            }
            self.create_tile(level, x, z);
        }
        self.tile_mut(level, x, z)
    }

    /// `levelTiles[plane][x][z] = new Tile(level)`: the single-tile
    /// allocation of the particle collision pass,
    /// without `createTile`'s plane fill.
    pub fn put_tile(&mut self, plane: usize, x: usize, z: usize, level: i32) {
        let i = self.index(plane, x, z);
        self.tiles[i] = Some(Tile::new(level));
    }

    /// `getTileCoerce`.
    pub fn get_tile_coerce(&mut self, level: usize, x: i32, z: i32) -> Option<&mut Tile> {
        let cx = (x.max(0) as usize).min(self.max_x - 1);
        let cz = (z.max(0) as usize).min(self.max_z - 1);
        self.get_tile(level, cx, cz)
    }

    /// `reset()`: every plane-0 tile exists.
    pub fn reset(&mut self) {
        for x in 0..self.max_x {
            for z in 0..self.max_z {
                let i = self.index(0, x, z);
                if self.tiles[i].is_none() {
                    self.tiles[i] = Some(Tile::new(0));
                }
            }
        }
    }

    /// Turn the column at `(x, z)` into a bridge: the tiles above plane 0 move down one
    /// plane, and the old plane-0 tile stays under the new plane-0 tile as its `bridge`.
    pub fn set_bridge(&mut self, x: usize, z: usize) {
        let i0 = self.index(0, x, z);
        let ground_tile = self.tiles[i0].take();
        for from_plane in 0..3 {
            let ia = self.index(from_plane + 1, x, z);
            let above = self.tiles[ia].take();
            let i = self.index(from_plane, x, z);
            self.tiles[i] = above;
            if let Some(shifted_tile) = self.tiles[i].clone() {
                for &source in &shifted_tile.entities {
                    let PrimaryRef::Scenery(e) = source else {
                        continue;
                    };
                    let ent = &mut self.scenery[e];
                    if ent.min_tx == x as i32 && ent.min_tz == z as i32 {
                        ent.level -= 1;
                    }
                }
                if let Some(g) = shifted_tile.ground_decoration {
                    self.ground_decors[g].level -= 1;
                }
                if let Some(w) = shifted_tile.wall {
                    self.walls[w].level -= 1;
                }
                if let Some(w) = shifted_tile.dynamic_wall {
                    self.walls[w].level -= 1;
                }
                if let Some(d) = shifted_tile.wall_decoration {
                    self.wall_decors[d].level -= 1;
                }
                if let Some(d) = shifted_tile.dynamic_wall_decoration {
                    self.wall_decors[d].level -= 1;
                }
            }
        }
        let i0 = self.index(0, x, z);
        if self.tiles[i0].is_none() {
            let mut t = Tile::new(0);
            t.level = 1;
            self.tiles[i0] = Some(t);
        }
        self.tiles[i0].as_mut().expect("plane 0").bridge = ground_tile.map(Box::new);
        let i3 = self.index(3, x, z);
        self.tiles[i3] = None;
    }

    fn bucket(&mut self, e: EntityRef, model_ready: bool, transparent: bool) {
        if !model_ready {
            self.pending.push(e);
        } else if transparent {
            self.transparent.push(e);
        } else {
            self.opaque.push(e);
        }
    }

    /// Add a ground decoration to the tile at `(level, x, z)`.
    pub fn add_ground_decoration(
        &mut self,
        level: usize,
        x: usize,
        z: usize,
        entity: GroundDecorEntity,
    ) {
        let ready = !entity.dynamic && entity.model.as_ref().is_none_or(|m| !m.has_animated_uvs);
        let transparent =
            entity.dynamic || entity.model.as_ref().is_some_and(|m| m.has_transparency);
        let id = self.ground_decors.len();
        let Some(tile) = self.get_tile(level, x, z) else {
            return;
        };
        tile.ground_decoration = Some(id);
        self.ground_decors.push(entity);
        self.bucket(EntityRef::GroundDecor(id), ready, transparent);
    }

    /// Add a wall (and the second wall of an L corner) to the tile at `(level, x, z)`.
    pub fn add_wall(
        &mut self,
        level: usize,
        x: usize,
        z: usize,
        wall: WallEntity,
        wall2: Option<WallEntity>,
    ) {
        let ready = !wall.dynamic && wall.model.as_ref().is_none_or(|m| !m.has_animated_uvs);
        let transparent = wall.dynamic || wall.model.as_ref().is_some_and(|m| m.has_transparency);
        let ready2 = wall2
            .as_ref()
            .map(|w| !w.dynamic && w.model.as_ref().is_none_or(|m| !m.has_animated_uvs));
        let transparent2 = wall2
            .as_ref()
            .map(|w| w.dynamic || w.model.as_ref().is_some_and(|m| m.has_transparency));
        if self.get_tile(level, x, z).is_none() {
            return;
        }
        let id = self.walls.len();
        self.walls.push(wall);
        let id2 = wall2.map(|w| {
            let id2 = self.walls.len();
            self.walls.push(w);
            id2
        });
        let tile = self.tile_mut(level, x, z).expect("tile");
        tile.wall = Some(id);
        tile.dynamic_wall = id2;
        self.bucket(EntityRef::Wall(id), ready, transparent);
        if let Some(id2) = id2 {
            self.bucket(
                EntityRef::Wall(id2),
                ready2.expect("wall2"),
                transparent2.expect("wall2"),
            );
        }
    }

    /// Add a wall decoration (and a second one for the both-sides shape) to the tile at
    /// `(level, x, z)`.
    pub fn add_wall_decoration(
        &mut self,
        level: usize,
        x: usize,
        z: usize,
        decor: WallDecorEntity,
        decor2: Option<WallDecorEntity>,
    ) {
        let ready = !decor.dynamic && decor.model.as_ref().is_none_or(|m| !m.has_animated_uvs);
        let transparent = decor.dynamic || decor.model.as_ref().is_some_and(|m| m.has_transparency);
        let ready2 = decor2
            .as_ref()
            .map(|w| !w.dynamic && w.model.as_ref().is_none_or(|m| !m.has_animated_uvs));
        let transparent2 = decor2
            .as_ref()
            .map(|w| w.dynamic || w.model.as_ref().is_some_and(|m| m.has_transparency));
        if self.get_tile(level, x, z).is_none() {
            return;
        }
        let id = self.wall_decors.len();
        self.wall_decors.push(decor);
        let id2 = decor2.map(|w| {
            let id2 = self.wall_decors.len();
            self.wall_decors.push(w);
            id2
        });
        let tile = self.tile_mut(level, x, z).expect("tile");
        tile.wall_decoration = Some(id);
        tile.dynamic_wall_decoration = id2;
        self.bucket(EntityRef::WallDecor(id), ready, transparent);
        if let Some(id2) = id2 {
            self.bucket(
                EntityRef::WallDecor(id2),
                ready2.expect("decor2"),
                transparent2.expect("decor2"),
            );
        }
    }

    /// Add a static loc entity: append to every footprint tile, then bucket. Always
    /// returns true. The underwater water-fog propagation is not
    /// modelled (normal set only).
    pub fn add_entity(&mut self, entity: SceneryEntity) -> bool {
        let ready = !entity.dynamic && entity.model.as_ref().is_none_or(|m| !m.has_animated_uvs);
        let transparent =
            entity.dynamic || entity.model.as_ref().is_some_and(|m| m.has_transparency);
        let id = self.scenery.len();
        let level = entity.level as usize;
        let clamped_min_x = (entity.min_tx.max(0)).min(self.max_x as i32 - 1);
        let clamped_max_x = (entity.max_tx.max(0)).min(self.max_x as i32 - 1);
        let clamped_min_z = (entity.min_tz.max(0)).min(self.max_z as i32 - 1);
        let clamped_max_z = (entity.max_tz.max(0)).min(self.max_z as i32 - 1);
        self.scenery.push(entity);
        for tile_x in clamped_min_x..=clamped_max_x {
            for tile_z in clamped_min_z..=clamped_max_z {
                if let Some(tile) = self.get_tile_coerce(level, tile_x, tile_z) {
                    tile.entities.push(PrimaryRef::Scenery(id));
                }
            }
        }
        self.bucket(EntityRef::Scenery(id), ready, transparent);
        true
    }

    /// Scale the offsets of the wall decorations on a tile by the wall's `walloff`.
    pub fn set_wall_decoration_offset(
        &mut self,
        level: usize,
        x: usize,
        z: usize,
        wall_offset: i32,
    ) {
        let Some(tile) = self.tile(level, x, z) else {
            return;
        };
        let (wall_decor_index, dynamic_wall_decor_index) =
            (tile.wall_decoration, tile.dynamic_wall_decoration);
        let div = 0x10 << (self.size - 7);
        for d in [wall_decor_index, dynamic_wall_decor_index]
            .into_iter()
            .flatten()
        {
            let e = &mut self.wall_decors[d];
            e.offset_x = ((e.offset_x * wall_offset / div) as i16) as i32;
            e.offset_z = ((e.offset_z * wall_offset / div) as i16) as i32;
        }
    }

    /// The wall of the tile at `(level, x, z)`.
    #[must_use]
    pub fn get_wall(&self, level: usize, x: usize, z: usize) -> Option<&WallEntity> {
        self.tile(level, x, z)
            .and_then(|t| t.wall)
            .map(|w| &self.walls[w])
    }

    // -- normal merging -----------------------------------------------------

    fn needs_merge(&self, e: EntityRef) -> bool {
        match e {
            EntityRef::Scenery(i) => self.scenery[i].use_merged_normals,
            EntityRef::Wall(i) => self.walls[i].use_merged_normals,
            EntityRef::WallDecor(_) | EntityRef::Temporary(_) => false,
            EntityRef::GroundDecor(i) => self.ground_decors[i].use_merged_normals,
        }
    }

    fn apply_lighting(&mut self, e: EntityRef) {
        match e {
            EntityRef::Scenery(i) => self.scenery[i].use_merged_normals = false,
            EntityRef::Wall(i) => self.walls[i].use_merged_normals = false,
            EntityRef::WallDecor(_) | EntityRef::Temporary(_) => {}
            EntityRef::GroundDecor(i) => self.ground_decors[i].use_merged_normals = false,
        }
    }

    fn take_model(&mut self, e: EntityRef) -> Option<GpuModel> {
        match e {
            EntityRef::Scenery(i) => self.scenery[i].model.take(),
            EntityRef::Wall(i) => self.walls[i].model.take(),
            EntityRef::WallDecor(i) => self.wall_decors[i].model.take(),
            EntityRef::GroundDecor(i) => self.ground_decors[i].model.take(),
            EntityRef::Temporary(i) => self.temporary[i].model.take(),
        }
    }

    fn put_model(&mut self, e: EntityRef, m: Option<GpuModel>) {
        match e {
            EntityRef::Scenery(i) => self.scenery[i].model = m,
            EntityRef::Wall(i) => self.walls[i].model = m,
            EntityRef::WallDecor(i) => self.wall_decors[i].model = m,
            EntityRef::GroundDecor(i) => self.ground_decors[i].model = m,
            EntityRef::Temporary(i) => self.temporary[i].model = m,
        }
    }

    /// Merge normals of `b` into `a`: the static entities only merge scenery/wall with
    /// scenery/wall and ground decor with ground decor.
    fn merge_pair(&mut self, a: EntityRef, b: EntityRef, dx: i32, dy: i32, dz: i32) {
        if a == b {
            return;
        }
        let compatible = matches!(
            (a, b),
            (
                EntityRef::Scenery(_) | EntityRef::Wall(_),
                EntityRef::Scenery(_) | EntityRef::Wall(_),
            ) | (EntityRef::GroundDecor(_), EntityRef::GroundDecor(_))
        );
        if !compatible {
            return;
        }
        let ma = self.take_model(a);
        let mb = self.take_model(b);
        let (mut ma, mut mb) = match (ma, mb) {
            (Some(ma), Some(mb)) => (ma, mb),
            (ma, mb) => {
                self.put_model(a, ma);
                self.put_model(b, mb);
                return;
            }
        };
        ma.merge_normals(&mut mb, dx, dy, dz);
        self.put_model(a, Some(ma));
        self.put_model(b, Some(mb));
    }

    fn corner_avg(heights: &FloorHeights, x: i32, z: i32) -> i32 {
        let h = |x: i32, z: i32| heights.get_tile_height(x as usize, z as usize);
        (h(x, z) + h(x + 1, z) + h(x, z + 1) + h(x + 1, z + 1)) / 4
    }

    /// Merge normals and apply lighting for every placed loc. `heights[level]` are
    /// the floor heights of each level (all planes present).
    pub fn build_models(&mut self, heights: &[FloorHeights]) {
        for level in 0..self.max_level {
            for tile_x in 0..self.max_x {
                for tile_z in 0..self.max_z {
                    let Some(tile) = self.tile(level, tile_x, tile_z) else {
                        continue;
                    };
                    let (wall_index, dynamic_wall_index, ents, ground_decor_index) = (
                        tile.wall,
                        tile.dynamic_wall,
                        tile.entities.clone(),
                        tile.ground_decoration,
                    );
                    if let Some(w) = wall_index {
                        let wr = EntityRef::Wall(w);
                        if self.needs_merge(wr) {
                            self.merge_loc_normals(
                                wr,
                                level,
                                [tile_x as i32, tile_z as i32],
                                [1, 1],
                                heights,
                            );
                            if let Some(w2) = dynamic_wall_index {
                                let wr2 = EntityRef::Wall(w2);
                                if self.needs_merge(wr2) {
                                    self.merge_loc_normals(
                                        wr2,
                                        level,
                                        [tile_x as i32, tile_z as i32],
                                        [1, 1],
                                        heights,
                                    );
                                    self.merge_pair(wr2, wr, 0, 0, 0);
                                    self.apply_lighting(wr2);
                                }
                            }
                            self.apply_lighting(wr);
                        }
                    }
                    for source in ents {
                        let PrimaryRef::Scenery(e) = source else {
                            continue;
                        };
                        let er = EntityRef::Scenery(e);
                        if self.needs_merge(er) {
                            let ent = &self.scenery[e];
                            let w = ent.max_tx - ent.min_tx + 1;
                            let l = ent.max_tz - ent.min_tz + 1;
                            self.merge_loc_normals(
                                er,
                                level,
                                [tile_x as i32, tile_z as i32],
                                [w, l],
                                heights,
                            );
                            self.apply_lighting(er);
                        }
                    }
                    if let Some(g) = ground_decor_index {
                        let gr = EntityRef::GroundDecor(g);
                        if self.needs_merge(gr) {
                            self.merge_ground_decoration_normals(
                                gr,
                                level,
                                tile_x as i32,
                                tile_z as i32,
                                heights,
                            );
                            self.apply_lighting(gr);
                        }
                    }
                }
            }
        }
    }

    /// Merge a ground decoration's normals with those of its neighbours.
    fn merge_ground_decoration_normals(
        &mut self,
        merging_entity: EntityRef,
        level: usize,
        tile_x: i32,
        tile_z: i32,
        heights: &[FloorHeights],
    ) {
        let hm = &heights[level];
        let base = Self::corner_avg(hm, tile_x, tile_z);
        let ts = self.tile_size;
        // Both x and z are deliberately compared against the x extent.
        let mx = self.max_x as i32;
        let mz = self.max_z as i32;
        let neighbour = |s: &Self, x: i32, z: i32| -> Option<usize> {
            s.tile(level, x as usize, z as usize)
                .and_then(|t| t.ground_decoration)
                .filter(|&g| s.ground_decors[g].use_merged_normals)
        };
        if tile_x < mx {
            if let Some(g) = neighbour(self, tile_x + 1, tile_z) {
                let height_delta_next_x = Self::corner_avg(hm, tile_x + 1, tile_z) - base;
                self.merge_pair(
                    merging_entity,
                    EntityRef::GroundDecor(g),
                    ts,
                    height_delta_next_x,
                    0,
                );
            }
        }
        if tile_z < mx {
            if let Some(g) = neighbour(self, tile_x, tile_z + 1) {
                // The corners summed are (x, z), (x+1, z+1), (x, z+2), (x+1, z+2):
                // the first is (x, z), not (x, z+1). Keep the quirk.
                let h = |x: i32, z: i32| hm.get_tile_height(x as usize, z as usize);
                let height_delta_next_z = (h(tile_x, tile_z)
                    + h(tile_x + 1, tile_z + 1)
                    + h(tile_x, tile_z + 2)
                    + h(tile_x + 1, tile_z + 2))
                    / 4
                    - base;
                self.merge_pair(
                    merging_entity,
                    EntityRef::GroundDecor(g),
                    0,
                    height_delta_next_z,
                    ts,
                );
            }
        }
        if tile_x < mx && tile_z < mz {
            if let Some(g) = neighbour(self, tile_x + 1, tile_z + 1) {
                let height_delta_next_xz = Self::corner_avg(hm, tile_x + 1, tile_z + 1) - base;
                self.merge_pair(
                    merging_entity,
                    EntityRef::GroundDecor(g),
                    ts,
                    height_delta_next_xz,
                    ts,
                );
            }
        }
        if tile_x >= mx || tile_z <= 0 {
            return;
        }
        if let Some(g) = neighbour(self, tile_x + 1, tile_z - 1) {
            let height_delta_next_x_prev_z = Self::corner_avg(hm, tile_x + 1, tile_z - 1) - base;
            self.merge_pair(
                merging_entity,
                EntityRef::GroundDecor(g),
                ts,
                height_delta_next_x_prev_z,
                -ts,
            );
        }
    }

    /// Merge a loc's normals with those of the locs around its footprint.
    fn merge_loc_normals(
        &mut self,
        merging_entity: EntityRef,
        base_level: usize,
        tile: [i32; 2],
        size: [i32; 2],
        heights: &[FloorHeights],
    ) {
        let [tile_x, tile_z] = tile;
        let [size_x, size_z] = size;
        let mut first_plane_pass = true;
        let mut scan_min_x = tile_x;
        let scan_max_x = tile_x + size_x;
        let scan_min_z = tile_z - 1;
        let scan_max_z = tile_z + size_z;
        let base = Self::corner_avg(&heights[base_level], tile_x, tile_z);
        let half = self.half_tile_size;
        let ts = self.tile_size;
        #[allow(
            clippy::needless_range_loop,
            reason = "level loop: `scan_level` is also compared with the level count and selects tiles"
        )]
        for scan_level in base_level..=base_level + 1 {
            if self.max_level == scan_level {
                continue;
            }
            for scan_x in scan_min_x..=scan_max_x {
                if scan_x < 0 || scan_x >= self.max_x as i32 {
                    continue;
                }
                for scan_z in scan_min_z..=scan_max_z {
                    if scan_z < 0 || scan_z >= self.max_z as i32 {
                        continue;
                    }
                    if !(!first_plane_pass
                        || scan_x >= scan_max_x
                        || scan_z >= scan_max_z
                        || (scan_z < tile_z && tile_x != scan_x))
                    {
                        continue;
                    }
                    let Some(tile) = self.tile(scan_level, scan_x as usize, scan_z as usize) else {
                        continue;
                    };
                    let height_delta =
                        Self::corner_avg(&heights[scan_level], scan_x, scan_z) - base;
                    let (wall_index, dynamic_wall_index, ents) =
                        (tile.wall, tile.dynamic_wall, tile.entities.clone());
                    let dx = half * (1 - size_x) + ts * (scan_x - tile_x);
                    let dz = half * (1 - size_z) + ts * (scan_z - tile_z);
                    if let Some(w) = wall_index {
                        if self.walls[w].use_merged_normals {
                            self.merge_pair(
                                merging_entity,
                                EntityRef::Wall(w),
                                dx,
                                height_delta,
                                dz,
                            );
                        }
                    }
                    if let Some(w) = dynamic_wall_index {
                        if self.walls[w].use_merged_normals {
                            self.merge_pair(
                                merging_entity,
                                EntityRef::Wall(w),
                                dx,
                                height_delta,
                                dz,
                            );
                        }
                    }
                    for source in ents {
                        let PrimaryRef::Scenery(e) = source else {
                            continue;
                        };
                        let neighbour_entity = &self.scenery[e];
                        if neighbour_entity.use_merged_normals
                            && (neighbour_entity.min_tx == scan_x || scan_min_x == scan_x)
                            && (neighbour_entity.min_tz == scan_z || scan_min_z == scan_z)
                        {
                            let neighbour_size_x =
                                neighbour_entity.max_tx - neighbour_entity.min_tx + 1;
                            let neighbour_size_z =
                                neighbour_entity.max_tz - neighbour_entity.min_tz + 1;
                            let ex = half * (neighbour_size_x - size_x)
                                + ts * (neighbour_entity.min_tx - tile_x);
                            let ez = half * (neighbour_size_z - size_z)
                                + ts * (neighbour_entity.min_tz - tile_z);
                            self.merge_pair(
                                merging_entity,
                                EntityRef::Scenery(e),
                                ex,
                                height_delta,
                                ez,
                            );
                        }
                    }
                }
            }
            scan_min_x -= 1;
            first_plane_pass = false;
        }
    }

    // -- dump ---------------------------------------------------------------

    /// Serialise every tile and static entity (with its upload streams) for
    /// diffing against the committed scene dump. All integers are big-endian.
    pub fn to_dump(&self, materials: &crate::texture::MaterialStore) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::new();
        let w32 = |o: &mut Vec<u8>, v: i32| o.extend_from_slice(&v.to_be_bytes());
        let wf = |o: &mut Vec<u8>, v: f32| o.extend_from_slice(&v.to_bits().to_be_bytes());
        out.extend_from_slice(b"SCN1");
        w32(&mut out, self.max_level as i32);
        w32(&mut out, self.max_x as i32);
        w32(&mut out, self.max_z as i32);
        // tiles: u8 per (level, x, z): 0xFF = absent, else level | bridge<<7
        for level in 0..self.max_level {
            for x in 0..self.max_x {
                for z in 0..self.max_z {
                    out.push(match self.tile(level, x, z) {
                        None => 0xFF,
                        Some(t) => (t.level as u8) | if t.bridge.is_some() { 0x80 } else { 0 },
                    });
                }
            }
        }
        // entities in scan order
        let mut records: Vec<u8> = Vec::new();
        let mut count = 0_i32;
        let push_model = |rec: &mut Vec<u8>, model: Option<&GpuModel>| -> anyhow::Result<()> {
            match model {
                None => rec.push(0),
                Some(m) => {
                    rec.push(1);
                    rec.push(u8::from(m.has_transparency));
                    rec.push(u8::from(m.has_animated_uvs));
                    w32(rec, i32::from(m.ambient));
                    w32(rec, i32::from(m.contrast));
                    w32(rec, m.vertex_count);
                    w32(rec, m.unique_count);
                    w32(rec, m.face_count);
                    w32(rec, m.draw_face_count);
                    for p in m.position_stream() {
                        for c in p {
                            wf(rec, c);
                        }
                    }
                    for c in m.colour_stream(materials)? {
                        w32(rec, c);
                    }
                    for n in m.normal_stream() {
                        for c in n {
                            wf(rec, c);
                        }
                    }
                    // raw (merged-or-base) normal shorts + counts, for debugging
                    let (nx, ny, nz, nc): (&[i16], &[i16], &[i16], &[i8]) = match &m.merged {
                        Some(mm) => (&mm.nx, &mm.ny, &mm.nz, &mm.count),
                        None => (&m.nx, &m.ny, &m.nz, &m.ncount),
                    };
                    for i in 0..m.unique_count as usize {
                        rec.extend_from_slice(&nx[i].to_be_bytes());
                        rec.extend_from_slice(&ny[i].to_be_bytes());
                        rec.extend_from_slice(&nz[i].to_be_bytes());
                        rec.push(nc[i] as u8);
                    }
                    for uv in m.uv_stream() {
                        wf(rec, uv[0]);
                        wf(rec, uv[1]);
                    }
                    for i in m.index_stream() {
                        rec.extend_from_slice(&i.to_be_bytes());
                    }
                    let batches = m.batches();
                    w32(rec, batches.len() as i32);
                    for (mat, start, n, minv, span) in batches {
                        w32(rec, i32::from(mat));
                        w32(rec, start);
                        w32(rec, n);
                        w32(rec, minv);
                        w32(rec, span);
                    }
                }
            }
            Ok(())
        };
        // rot (xyzw) + scale (xyz) of the entity transform.
        let push_srt = |rec: &mut Vec<u8>, srt: Option<LocSrt>| {
            let (rot, scale) = match srt {
                Some(s) => (s.rot, s.scale),
                None => ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0]),
            };
            for v in rot.iter().chain(scale.iter()) {
                wf(rec, *v);
            }
        };
        for level in 0..self.max_level {
            for x in 0..self.max_x {
                for z in 0..self.max_z {
                    let Some(tile) = self.tile(level, x, z) else {
                        continue;
                    };
                    let head = |rec: &mut Vec<u8>, layer: u8| {
                        rec.push(layer);
                        rec.push(level as u8);
                        rec.extend_from_slice(&(x as u16).to_be_bytes());
                        rec.extend_from_slice(&(z as u16).to_be_bytes());
                    };
                    for (layer, w) in [(0_u8, tile.wall), (1, tile.dynamic_wall)] {
                        if let Some(w) = w {
                            let e = &self.walls[w];
                            if e.dynamic {
                                continue;
                            }
                            head(&mut records, layer);
                            for v in [
                                e.loc_id as i32,
                                e.shape,
                                e.angle,
                                e.level,
                                e.occlude_level,
                                e.x,
                                e.y,
                                e.z,
                                e.wall_type,
                                0,
                                0,
                                0,
                            ] {
                                w32(&mut records, v);
                            }
                            push_srt(&mut records, e.srt);
                            records.push(u8::from(e.active));
                            push_model(&mut records, e.model.as_ref())?;
                            count += 1;
                        }
                    }
                    for (layer, d) in [
                        (2_u8, tile.wall_decoration),
                        (3, tile.dynamic_wall_decoration),
                    ] {
                        if let Some(d) = d {
                            let e = &self.wall_decors[d];
                            if e.dynamic {
                                continue;
                            }
                            head(&mut records, layer);
                            for v in [
                                e.loc_id as i32,
                                e.shape,
                                e.angle,
                                e.level,
                                e.occlude_level,
                                e.x,
                                e.y,
                                e.z,
                                e.offset_x,
                                e.offset_z,
                                0,
                                0,
                            ] {
                                w32(&mut records, v);
                            }
                            push_srt(&mut records, e.srt);
                            records.push(u8::from(e.active));
                            push_model(&mut records, e.model.as_ref())?;
                            count += 1;
                        }
                    }
                    if let Some(g) = tile
                        .ground_decoration
                        .filter(|&g| !self.ground_decors[g].dynamic)
                    {
                        let e = &self.ground_decors[g];
                        head(&mut records, 4);
                        for v in [
                            e.loc_id as i32,
                            22,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            e.decor_height,
                            0,
                            0,
                            0,
                        ] {
                            w32(&mut records, v);
                        }
                        push_srt(&mut records, e.srt);
                        records.push(u8::from(e.active));
                        push_model(&mut records, e.model.as_ref())?;
                        count += 1;
                    }
                    for &source in &tile.entities {
                        let PrimaryRef::Scenery(i) = source else {
                            continue;
                        };
                        let e = &self.scenery[i];
                        if e.dynamic || e.min_tx != x as i32 || e.min_tz != z as i32 {
                            continue;
                        }
                        head(&mut records, 5);
                        for v in [
                            e.loc_id as i32,
                            e.shape,
                            e.angle,
                            e.level,
                            e.occlude_level,
                            e.x,
                            e.y,
                            e.z,
                            e.min_tx,
                            e.max_tx,
                            e.min_tz,
                            e.max_tz,
                        ] {
                            w32(&mut records, v);
                        }
                        push_srt(&mut records, e.srt);
                        records.push(
                            u8::from(e.active)
                                | (u8::from(e.raised) << 1)
                                | (u8::from(e.primary_layer) << 2)
                                | ((e.diag as u8) << 3),
                        );
                        push_model(&mut records, e.model.as_ref())?;
                        count += 1;
                    }
                }
            }
        }
        w32(&mut out, count);
        out.extend_from_slice(&records);
        Ok(out)
    }
}

impl SceneryEntity {
    fn render_metadata(&self) -> Self {
        Self {
            level: self.level,
            occlude_level: self.occlude_level,
            x: self.x,
            y: self.y,
            z: self.z,
            min_tx: self.min_tx,
            max_tx: self.max_tx,
            min_tz: self.min_tz,
            max_tz: self.max_tz,
            raised: self.raised,
            diag: self.diag,
            model_y: self.model_y,
            loc_id: self.loc_id,
            shape: self.shape,
            angle: self.angle,
            active: self.active,
            use_merged_normals: self.use_merged_normals,
            has_hard_shadow: self.has_hard_shadow,
            primary_layer: self.primary_layer,
            srt: self.srt,
            dynamic: self.dynamic,
            model: None,
        }
    }
}

impl WallEntity {
    fn render_metadata(&self) -> Self {
        Self {
            level: self.level,
            occlude_level: self.occlude_level,
            x: self.x,
            y: self.y,
            z: self.z,
            wall_type: self.wall_type,
            loc_id: self.loc_id,
            shape: self.shape,
            angle: self.angle,
            active: self.active,
            use_merged_normals: self.use_merged_normals,
            has_hard_shadow: self.has_hard_shadow,
            srt: self.srt,
            dynamic: self.dynamic,
            model: None,
        }
    }
}

impl WallDecorEntity {
    fn render_metadata(&self) -> Self {
        Self {
            level: self.level,
            occlude_level: self.occlude_level,
            x: self.x,
            y: self.y,
            z: self.z,
            offset_x: self.offset_x,
            offset_z: self.offset_z,
            loc_id: self.loc_id,
            shape: self.shape,
            angle: self.angle,
            active: self.active,
            has_hard_shadow: self.has_hard_shadow,
            srt: self.srt,
            dynamic: self.dynamic,
            model: None,
        }
    }
}

impl GroundDecorEntity {
    fn render_metadata(&self) -> Self {
        Self {
            level: self.level,
            occlude_level: self.occlude_level,
            x: self.x,
            y: self.y,
            z: self.z,
            decor_height: self.decor_height,
            loc_id: self.loc_id,
            angle: self.angle,
            active: self.active,
            use_merged_normals: self.use_merged_normals,
            has_hard_shadow: self.has_hard_shadow,
            srt: self.srt,
            dynamic: self.dynamic,
            model: None,
        }
    }
}

impl TemporaryEntity {
    fn render_metadata(&self) -> Self {
        Self {
            player: self.player,
            npc_index: self.npc_index,
            location_key: self.location_key,
            pick: self.pick,
            transient: self.transient,
            level: self.level,
            occlude_level: self.occlude_level,
            position: self.position,
            bounds: self.bounds,
            overlay_height: self.overlay_height,
            transparent: self.transparent,
            spot_shadow: self.spot_shadow,
            model: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_tile_fills_planes_below_and_bridges_bump_level() {
        let mut s = Scene::new(9, 4, 4, 4);
        s.create_tile(2, 1, 1);
        assert_eq!(s.tile(2, 1, 1).unwrap().level, 2);
        assert_eq!(s.tile(1, 1, 1).unwrap().level, 1);
        assert_eq!(s.tile(0, 1, 1).unwrap().level, 0);
        assert!(s.tile(3, 1, 1).is_none());
        s.set_bridge(1, 1);
        // plane 0 now holds the old plane-1 tile; its bridge is the old plane 0
        assert_eq!(s.tile(0, 1, 1).unwrap().level, 1);
        assert!(s.tile(0, 1, 1).unwrap().bridge.is_some());
        assert_eq!(s.tile(1, 1, 1).unwrap().level, 2);
        assert!(s.tile(2, 1, 1).is_none());
        // getTile refuses to auto-create plane 3 on a bridged column
        assert!(s.get_tile(3, 1, 1).is_none());
        s.create_tile(2, 1, 1);
        assert_eq!(s.tile(2, 1, 1).unwrap().level, 3);
    }

    #[test]
    fn wall_decoration_offset_scales_by_walloff() {
        let mut s = Scene::new(9, 4, 4, 4);
        let decor = WallDecorEntity {
            level: 0,
            occlude_level: 0,
            x: 256,
            y: 0,
            z: 256,
            offset_x: 65,
            offset_z: -65,
            loc_id: 1,
            shape: 5,
            angle: 0,
            active: false,
            has_hard_shadow: false,
            srt: None,
            dynamic: false,
            model: None,
        };
        s.add_wall_decoration(0, 1, 1, decor, None);
        s.set_wall_decoration_offset(0, 1, 1, 32);
        assert_eq!(s.wall_decors[0].offset_x, 65 * 32 / 64);
        assert_eq!(s.wall_decors[0].offset_z, -65 * 32 / 64);
        assert_eq!(s.pending.len(), 0);
        assert_eq!(s.opaque, vec![EntityRef::WallDecor(0)]);
    }
}
