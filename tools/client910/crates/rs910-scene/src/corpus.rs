//! Corpus check of the scene layer: several diverse regions of the local cache
//! are built through the path the client uses (landscape decode, floors,
//! underwater floors, loc placement, occluders, static lighting, the roof
//! state, the minimap plan, the draw planner and the occlusion raster) and
//! each stage is reduced to a hash. `fixtures/scene-corpus.tsv` holds one
//! digest per region and stage, so a change in what the scene layer produces
//! fails the test with the region and the stage named. The hash covers values
//! only, never field names, so renaming a field does not move a digest.
//!
//! Regions (see [`regions`]): a walled city, the same city at the lowest
//! graphics preferences, a dungeon, a coast at the highest water detail (the
//! underwater floor), a multi-level tower on the sea, a desert town, the
//! wilderness, two windows that reach the edge of the map (squares that do
//! not exist), and an instanced region assembled from rotated chunks of the
//! city.
//!
//! Stages (one row each per region; `count` is the number of things the
//! stage covers, so an empty stage is visible in the fixture):
//!
//! - `terrain`: the tile flags of every level, the occlusion map, the squares
//!   and the decoded locs.
//! - `floors/normal<n>`, `floors/underwater`: a finished floor, every field.
//! - `graph/tiles`, `graph/entities`, `graph/occluders`: the placed scene
//!   graph (its tiles, every placed loc with its model, the occluder and
//!   occlusion-map records) and its serialised dump.
//! - `lighting`: the baked static lights and the environment grid.
//! - `placement`: the placement counters, the positional loc sounds and the
//!   underwater models with their fog.
//! - `roof`: the roof stamp flood and the three roof modes over camera
//!   positions.
//! - `minimap`: the queued map elements and locs and the base plan of every
//!   level.
//! - `draw`: the draw decisions, occlusion raster and submissions of a set of
//!   cameras, and the underwater lists.
//!
//! The digest holds counts and hashes only, never cache data. Regenerate it
//! only for an intended change of a produced value: `RS910_UPDATE_CORPUS=1
//! cargo test -p rs910-scene --release corpus`. `RS910_CORPUS_REGIONS=a,b`
//! restricts a run to the named regions (an update then rewrites only those
//! rows).

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;

use crate::cache::Pack;
use crate::config::LocStore;
use crate::draw::{frame_traces, DrawFrame, DrawState, LiveInputs, PlannerScene};
use crate::draw_entity::DrawEntity;
use crate::flo::FloStore;
use crate::floor::FloorGeometry;
use crate::maploader::FloTables;
use crate::minimap::{Mark, Minimap};
use crate::protocol910::rebuild_state::{Kind, RegionLayout, World};
use crate::protocol910::terrain::{RegionCopy, Terrain};
use crate::rebuild::{
    rebuild_normal, rebuild_region_world, window_squares, BuildPrefs, Rebuild, MAP_SIZE_STANDARD,
};
use crate::roof::{RoofInput, RoofState, RoofWorld};
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

/// Values a digest can take, with length and presence marks so two different
/// shapes never produce the same byte stream.
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

/// The `Debug` text of a value without its struct and field names, hashed.
/// A renamed field leaves the digest alone; quoted strings pass through
/// unchanged. The text is formatted into `scratch`, which the caller reuses.
fn put_debug<T: Debug + ?Sized>(digest: &mut Digest, scratch: &mut String, value: &T) {
    use std::fmt::Write;
    scratch.clear();
    write!(scratch, "{value:?}").expect("formatting into a string");
    put_without_names(digest, scratch.as_bytes());
}

fn put_without_names(digest: &mut Digest, text: &[u8]) {
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let mut i = 0;
    while i < text.len() {
        let c = text[i];
        if c == b'"' {
            let start = i;
            i += 1;
            while i < text.len() {
                if text[i] == b'\\' {
                    i += 2;
                    continue;
                }
                i += 1;
                if text[i - 1] == b'"' {
                    break;
                }
            }
            digest.bytes(&text[start..i.min(text.len())]);
            continue;
        }
        let starts_word = (c.is_ascii_alphabetic() || c == b'_')
            && (i == 0 || !(is_word(text[i - 1]) || text[i - 1] == b'.'));
        if starts_word {
            let mut j = i;
            while j < text.len() && is_word(text[j]) {
                j += 1;
            }
            let field_name = text.get(j) == Some(&b':') && text.get(j + 1) == Some(&b' ');
            let type_name = text.get(j) == Some(&b' ')
                && text.get(j + 1) == Some(&b'{')
                && c.is_ascii_uppercase();
            if field_name {
                i = j + 2;
            } else if type_name {
                i = j + 1;
            } else {
                digest.bytes(&text[i..j]);
                i = j;
            }
            continue;
        }
        digest.bytes(&[c]);
        i += 1;
    }
}

// ---------------------------------------------------------------------------
// The regions
// ---------------------------------------------------------------------------

/// Where a region's scene comes from.
enum Source {
    /// The standard window centred on an absolute tile.
    Window { x: i32, z: i32 },
    /// A window assembled from rotated 8x8 chunks of the city squares.
    Instanced,
}

struct Region {
    name: &'static str,
    source: Source,
    prefs: BuildPrefs,
}

/// The lowest graphics preferences: no textures, no lighting detail, no
/// shadows, no ground blending or decoration.
const LOW: BuildPrefs = BuildPrefs {
    scenery_shadows: 0,
    water_detail: 0,
    lighting_detail: 0,
    ground_blending: 0,
    textures: 0,
    brightness: 1,
    ground_decoration: 0,
    anim_detail: 0,
};

/// The highest: the underwater floor is built where a square has one.
const HIGH: BuildPrefs = BuildPrefs {
    scenery_shadows: 2,
    water_detail: 2,
    lighting_detail: 1,
    ground_blending: 1,
    textures: 1,
    brightness: 4,
    ground_decoration: 1,
    anim_detail: 1,
};

fn regions() -> Vec<Region> {
    let window = |name, x, z, prefs| Region {
        name,
        source: Source::Window { x, z },
        prefs,
    };
    let default = BuildPrefs::default();
    vec![
        window("city", 3212, 3428, default),
        window("city-low", 3212, 3428, LOW),
        window("dungeon", 2884, 9800, default),
        window("coast", 3030, 3220, HIGH),
        window("tower", 3109, 3162, HIGH),
        window("desert", 3428, 2916, default),
        window("wilderness", 3095, 3660, default),
        window("map-edge", 3352, 4180, default),
        window("map-edge-dense", 2392, 4276, default),
        Region {
            name: "instanced",
            source: Source::Instanced,
            prefs: default,
        },
    ]
}

/// The 104x104 instanced window: destination chunk `(cx, cz)` of plane `l`
/// takes a source chunk of the city squares with a rotation that varies by
/// chunk; some chunks stay empty, and the upper planes are only partly filled.
fn instanced_world(pack: &Pack) -> (World, RegionLayout) {
    let chunks = MAP_SIZE_STANDARD / 8;
    let squares = [(50_i32, 53_i32), (51, 53), (50, 54)];
    let mut templates = Vec::new();
    for level in 0..4_i32 {
        for cx in 0..chunks as i32 {
            for cz in 0..chunks as i32 {
                let empty = (cx * 7 + cz * 3 + level) % 11 == 0 || (level >= 2 && cx >= 6);
                if empty {
                    templates.push(-1);
                    continue;
                }
                let src_x = 400 + (cx * 5 + cz) % 16;
                let src_z = 424 + (cx + cz * 3) % 12;
                let rotation = (cx + cz + level) % 4;
                templates.push((level << 24) | (src_x << 14) | (src_z << 3) | (rotation << 1));
            }
        }
    }
    let map_squares: Vec<i32> = squares.iter().map(|&(x, z)| (x << 8) | z).collect();
    let groups: Vec<i32> = squares.iter().map(|&(x, z)| x | (z << 7)).collect();
    for &group in &groups {
        pack.read_group(crate::map::MAP_ARCHIVE, group as u32)
            .expect("city square present");
    }
    let world = World {
        base_x: 6400,
        base_z: 5000,
        region_x: 6400 / 8 + 6,
        region_z: 5000 / 8 + 6,
        width: MAP_SIZE_STANDARD as i32,
        height: MAP_SIZE_STANDARD as i32,
        area: None,
        last_kind: Kind::Region,
        npc_bits: 0,
        group_count: groups.len(),
        map_squares,
        groups,
    };
    (
        world,
        RegionLayout {
            chunks_x: chunks,
            chunks_z: chunks,
            templates,
        },
    )
}

// ---------------------------------------------------------------------------
// One region's build
// ---------------------------------------------------------------------------

struct Stores {
    pack: Pack,
    tables: FloTables,
    materials: MaterialStore,
    locs: LocStore,
}

impl Stores {
    fn load(pack: &Pack) -> Self {
        let flo = FloStore::load(pack).expect("floors");
        Self {
            pack: pack.clone(),
            tables: FloTables::from_store(&flo),
            materials: MaterialStore::load(pack).expect("materials"),
            locs: LocStore::load(pack).expect("locs"),
        }
    }
}

/// The client's terrain of the window (what the minimap and roofs read),
/// decoded from the same LAND files the build used.
fn window_terrain(pack: &Pack, built: &Rebuild) -> Terrain {
    let mut terrain = Terrain::new(built.map_size, built.map_size).expect("terrain");
    for &square in &built.squares {
        let (mx, mz) = ((square >> 8) as i32, (square & 0xFF) as i32);
        let group = (square >> 8) | ((square & 0xFF) << 7);
        let Some(land) = pack
            .read_group(crate::map::MAP_ARCHIVE, group)
            .ok()
            .and_then(|mut files| files.remove(&crate::cache::LAND_FILE))
        else {
            continue;
        };
        terrain = terrain
            .read_normal(
                &land,
                mx * 64 - built.base_x,
                mz * 64 - built.base_z,
                built.base_x,
                built.base_z,
            )
            .expect("land decodes")
            .state;
    }
    terrain
}

fn region_terrain(pack: &Pack, world: &World, layout: &RegionLayout) -> Terrain {
    let mut terrain = Terrain::new(world.width as usize, world.height as usize).expect("terrain");
    for level in 0..4 {
        for cx in 0..layout.chunks_x {
            for cz in 0..layout.chunks_z {
                let template =
                    layout.templates[(level * layout.chunks_x + cx) * layout.chunks_z + cz];
                if template < 0 {
                    continue;
                }
                let (src_x, src_z) = ((template >> 14) & 0x3ff, (template >> 3) & 0x7ff);
                let group = (src_x >> 3) | ((src_z >> 3) << 7);
                let Some(land) = pack
                    .read_group(crate::map::MAP_ARCHIVE, group as u32)
                    .ok()
                    .and_then(|mut files| files.remove(&crate::cache::LAND_FILE))
                else {
                    continue;
                };
                terrain = terrain
                    .read_region(
                        &land,
                        RegionCopy {
                            level,
                            tile_x: (cx * 8) as i32,
                            tile_z: (cz * 8) as i32,
                            src_level: ((template >> 24) & 3) as usize,
                            src_chunk_x: src_x,
                            src_chunk_z: src_z,
                            rotation: (template >> 1) & 3,
                        },
                    )
                    .expect("region land decodes")
                    .state;
            }
        }
    }
    terrain
}

/// Build one region and its terrain.
fn build(stores: &Stores, region: &Region) -> (Rebuild, Terrain) {
    match region.source {
        Source::Window { x, z } => {
            let built = rebuild_normal(
                &stores.pack,
                &stores.tables,
                &stores.materials,
                Some(&stores.locs),
                x,
                z,
                &region.prefs,
            )
            .unwrap_or_else(|error| panic!("{}: {error:#}", region.name));
            let terrain = window_terrain(&stores.pack, &built);
            (built, terrain)
        }
        Source::Instanced => {
            let (world, layout) = instanced_world(&stores.pack);
            let built = rebuild_region_world(
                &stores.pack,
                &stores.tables,
                &stores.materials,
                &stores.locs,
                &world,
                &layout,
                &region.prefs,
            )
            .unwrap_or_else(|error| panic!("{}: {error:#}", region.name));
            let terrain = region_terrain(&stores.pack, &world, &layout);
            (built, terrain)
        }
    }
}

// ---------------------------------------------------------------------------
// What each stage feeds
// ---------------------------------------------------------------------------

/// One stage's result.
struct Stage {
    name: String,
    count: u64,
    digest: u64,
}

struct Stages {
    list: Vec<Stage>,
    scratch: String,
}

impl Stages {
    fn add(
        &mut self,
        name: impl Into<String>,
        count: usize,
        feed: impl FnOnce(&mut Digest, &mut String),
    ) {
        let mut digest = Digest::new();
        feed(&mut digest, &mut self.scratch);
        self.list.push(Stage {
            name: name.into(),
            count: count as u64,
            digest: digest.0,
        });
    }
}

fn feed_model(d: &mut Digest, scratch: &mut String, m: &mut crate::gpumodel::GpuModel) {
    d.put(&m.flags);
    d.put(&m.detail);
    d.put(&m.ambient);
    d.put(&m.contrast);
    d.put(&m.vertex_count_all);
    d.put(&m.vertex_count);
    d.put(&m.vx);
    d.put(&m.vy);
    d.put(&m.vz);
    d.put(&m.vertex_source_models);
    put_debug(d, scratch, &m.vertex_groups);
    put_debug(d, scratch, &m.face_groups);
    d.put(&m.unique_count);
    d.put(&m.unique_vertex);
    d.put(&m.unique_face);
    d.put(&m.nx);
    d.put(&m.ny);
    d.put(&m.nz);
    d.put(&m.ncount);
    d.put(&m.u);
    d.put(&m.v);
    put_debug(d, scratch, &m.merged);
    d.put(&m.face_count);
    d.put(&m.draw_face_count);
    d.put(&m.face_colour);
    d.put(&m.face_alpha);
    d.put(&m.face_material);
    d.put(&m.face_part);
    d.put(&m.idx1);
    d.put(&m.idx2);
    d.put(&m.idx3);
    d.put(&m.vertex_offsets);
    d.put(&m.vertex_slots);
    d.put(&m.batch_face_start);
    d.put(&m.batch_min_vertex);
    d.put(&m.batch_vertex_span);
    d.put(&m.has_transparency);
    d.put(&m.has_animated_uvs);
    put_debug(d, scratch, &m.billboards);
    d.put(&m.face_source);
    d.put(&m.source_face_count);
    d.put(&m.source_face_priority);
    d.put(&m.has_particles);
    put_debug(d, scratch, &m.particle_emitters);
    put_debug(d, scratch, &m.particle_effectors);
    for bound in [
        m.min_x(),
        m.max_x(),
        m.min_y(),
        m.max_y(),
        m.min_z(),
        m.max_z(),
        m.horizontal_radius(),
        m.radius(),
        m.height(),
    ] {
        d.put(&bound);
    }
}

/// A draw entity as `Debug` text with its numeric fields in the trace order
/// (the layout the fixture was recorded with).
fn feed_draw_entity(d: &mut Digest, scratch: &mut String, e: &DrawEntity) {
    let text = format!(
        "DrawEntity {{ source: {:?}, dynamic: {:?}, bounds: {:?}, wall_type: {:?}, cylinder: {:?}, position: {:?}, precise_cylinder: {:?}, fields: {:?} }}",
        e.source, e.dynamic, e.bounds, e.wall_type, e.cylinder, e.position, e.precise_cylinder,
        e.words()
    );
    scratch.clear();
    scratch.push_str(&text);
    put_without_names(d, scratch.as_bytes());
}

fn feed_floor(d: &mut Digest, scratch: &mut String, floor: &Option<FloorGeometry>) {
    match floor {
        None => d.put(&false),
        Some(geometry) => {
            d.put(&true);
            d.bytes(&geometry.to_dump());
            put_debug(d, scratch, geometry);
        }
    }
}

fn feed_trace(d: &mut Digest, trace: &crate::draw_trace::Trace) {
    d.put(&trace.0.len());
    for (name, words) in &trace.0 {
        d.put(name.as_str());
        d.put(words);
    }
}

/// The eye height that puts a camera `above` fine units over the centre tile
/// of level 0.
fn eye_over_centre(built: &Rebuild, above: i32) -> i32 {
    let floor = built.scene.normal[0].as_ref().expect("level 0 floor");
    floor.heights.get_tile_height(52, 52) - above
}

/// The cameras of one region: the four compass views of the whole scene, a
/// sub-viewport view with roofs, low and high views, occlusion off and the
/// exclusion box, and the levels 1..3 profiles. Each row is
/// `[id, eye x, y, z, yaw, pitch, .. viewport .., clip planes, roof stamp,
/// level profile, frustum switch, roof mode, depth mode]`.
fn cameras(built: &Rebuild) -> Vec<DrawFrame> {
    let high = eye_over_centre(built, 4500);
    let low = eye_over_centre(built, 1200);
    let overview = eye_over_centre(built, 9000);
    let (cx, cz) = (26624, 26624);
    let row = |id, eye: [i32; 3], yaw, pitch, view: [i32; 4], tail: [i32; 5]| -> DrawFrame {
        DrawFrame::from_words([
            id, eye[0], eye[1], eye[2], yaw, pitch, 1001, 701, view[0], view[1], view[2], view[3],
            200, 14844, tail[0], tail[1], tail[2], tail[3], tail[4],
        ])
    };
    let full = [0, 0, 1001, 701];
    let part = [17, 23, 811, 577];
    vec![
        row(0, [cx, high, cz - 4096], 0, 1500, full, [-1, 1, 1, 0, 1]),
        row(1, [cx - 4096, high, cz], 4096, 1500, full, [-1, 1, 1, 0, 1]),
        row(2, [cx, high, cz + 4096], 8192, 1500, full, [-1, 1, 1, 0, 1]),
        row(
            3,
            [cx + 4096, high, cz],
            12288,
            1500,
            full,
            [-1, 1, 1, 0, 1],
        ),
        row(4, [cx, high, cz - 4096], 1733, 2787, part, [1, 1, 1, 0, 1]),
        row(
            5,
            [cx - 6144, low, cz - 6144],
            2048,
            900,
            full,
            [-1, 1, 1, 0, 1],
        ),
        row(
            6,
            [cx + 3000, low, cz + 1500],
            10000,
            1100,
            part,
            [128, 2, 1, 0, 1],
        ),
        row(7, [cx, overview, cz], 0, 2200, full, [-1, 1, 1, 0, 1]),
        row(
            8,
            [cx, overview, cz + 9000],
            8192,
            1800,
            full,
            [-1, 1, 1, 0, 2],
        ),
        row(9, [cx, high, cz - 4096], 0, 1500, full, [-1, 1, 0, 0, 1]),
        row(10, [cx, high, cz - 4096], 0, 1500, full, [1, 1, 1, 1, 1]),
        row(11, [cx, high, cz - 4096], 0, 1500, full, [1, 1, 1, 2, 1]),
        row(12, [cx, high, cz - 4096], 0, 1500, full, [1, 1, 1, 3, 1]),
        row(13, [cx, high, cz - 4096], 0, 1500, full, [-1, 0, 1, 0, 2]),
        row(14, [cx, high, cz - 4096], 0, 1500, full, [255, 1, 1, 0, 1]),
        row(15, [cx, high, cz - 4096], 0, 1500, full, [-1, 0, 1, 0, 0]),
    ]
}

/// The stages of one built region.
fn stages(stores: &Stores, built: &mut Rebuild, terrain: &Terrain) -> Vec<Stage> {
    let mut out = Stages {
        list: Vec::new(),
        scratch: String::new(),
    };
    let size = built.map_size;

    out.add("terrain", built.locs.len(), |d, scratch| {
        for level in 0..4 {
            for x in 0..size {
                for z in 0..size {
                    d.put(&built.flags.get(level, x, z));
                }
            }
        }
        d.put(&built.occludemap);
        d.put(&built.squares);
        d.put(&built.base_x);
        d.put(&built.base_z);
        d.put(&built.map_size);
        put_debug(d, scratch, &built.locs);
    });

    for level in 0..built.scene.normal.len() {
        let count = built.scene.normal[level]
            .as_ref()
            .map_or(0, |g| g.triangle_count());
        out.add(format!("floors/normal{level}"), count, |d, scratch| {
            feed_floor(d, scratch, &built.scene.normal[level]);
        });
    }
    let underwater = built.scene.underwater.first().and_then(Option::as_ref);
    out.add(
        "floors/underwater",
        underwater.map_or(0, |g| g.triangle_count()),
        |d, scratch| {
            feed_floor(
                d,
                scratch,
                &built.scene.underwater.first().cloned().flatten(),
            );
            put_debug(d, scratch, &built.scene.water_fog);
        },
    );

    let materials = &stores.materials;
    let heights: Vec<_> = built
        .scene
        .normal
        .iter()
        .map(|g| g.as_ref().expect("floor").heights.clone())
        .collect();
    {
        let graph = built.scene_graph.as_mut().expect("scene graph");
        let mut tiles = 0;
        out.add("graph/tiles", 0, |d, scratch| {
            for level in 0..graph.max_level {
                for x in 0..graph.max_x {
                    for z in 0..graph.max_z {
                        let tile = graph.tile(level, x, z);
                        tiles += usize::from(tile.is_some());
                        put_debug(d, scratch, &tile);
                    }
                }
            }
        });
        out.list.last_mut().unwrap().count = tiles as u64;
        let entity_count = graph.scenery.len()
            + graph.walls.len()
            + graph.wall_decors.len()
            + graph.ground_decors.len();
        out.add("graph/entities", entity_count, |d, scratch| {
            for e in &mut graph.scenery {
                let mut model = e.model.take();
                put_debug(d, scratch, e);
                if let Some(m) = model.as_mut() {
                    feed_model(d, scratch, m);
                }
                e.model = model;
            }
            for e in &mut graph.walls {
                let mut model = e.model.take();
                put_debug(d, scratch, e);
                if let Some(m) = model.as_mut() {
                    feed_model(d, scratch, m);
                }
                e.model = model;
            }
            for e in &mut graph.wall_decors {
                let mut model = e.model.take();
                put_debug(d, scratch, e);
                if let Some(m) = model.as_mut() {
                    feed_model(d, scratch, m);
                }
                e.model = model;
            }
            for e in &mut graph.ground_decors {
                let mut model = e.model.take();
                put_debug(d, scratch, e);
                if let Some(m) = model.as_mut() {
                    feed_model(d, scratch, m);
                }
                e.model = model;
            }
            put_debug(d, scratch, &graph.opaque);
            put_debug(d, scratch, &graph.transparent);
            put_debug(d, scratch, &graph.pending);
            d.bytes(&graph.to_dump(materials).expect("scene dump"));
        });
        out.add(
            "graph/occluders",
            graph.occluders.len() + graph.occlude_calls.len(),
            |d, scratch| {
                put_debug(d, scratch, &graph.occluders);
                put_debug(d, scratch, &graph.occlude_calls);
                let quads = crate::occlusion::build_occluders(graph, &heights);
                d.put(&quads.len());
                for q in &quads {
                    d.put(&q.words());
                }
            },
        );
    }

    let baked = built.lights.iter().map(Vec::len).sum();
    out.add("lighting", baked, |d, scratch| {
        for level in &built.lights {
            d.bytes(&crate::floorlight::to_dump(level));
            put_debug(d, scratch, level);
        }
        d.bytes(&built.env.to_dump());
        put_debug(d, scratch, &built.env);
    });

    out.add("placement", built.loc_stats.placed, |d, scratch| {
        put_debug(d, scratch, &built.loc_stats);
        put_debug(d, scratch, &built.loc_sounds.spawns);
        d.put(&built.model_ids);
        d.put(&built.underwater_models.len());
        for m in &mut built.underwater_models {
            let mut model = m.model.clone();
            feed_model(d, scratch, &mut model);
            d.put(&m.position);
            d.put(&m.transparent);
            d.put(&m.fog_plane);
            d.put(&m.fog_colour);
            d.put(&m.water_height);
            d.put(&m.water_colour);
            d.put(&m.water_scale);
            feed_draw_entity(d, scratch, &m.entity);
        }
    });
    out.list.last_mut().unwrap().count += built.underwater_models.len() as u64;

    // The roof state over the window: the flood of every roof of the level,
    // then each roof mode over camera and player positions.
    {
        let graph = built.scene_graph.as_ref().expect("scene graph");
        let world = RoofWorld {
            scene: Some(graph),
            flags: &built.flags,
            heights: &heights,
        };
        let mut marked = 0_usize;
        out.add("roof", 0, |d, scratch| {
            let centre = 52 * 512 + 256;
            for level in 0..3 {
                let mut state = RoofState::default();
                state.setup(&world, 1, level, 100);
                d.put(&state.stamps);
                d.put(&state.boxes);
                marked += state
                    .stamps
                    .iter()
                    .flatten()
                    .flatten()
                    .flatten()
                    .filter(|&&s| s != 0)
                    .count();
                for mode in [2, 3] {
                    let mut state = RoofState::default();
                    state.setup(&world, mode, level, 100);
                    for (n, (camera_state, dx, dz, pitch)) in [
                        (0, 0, 0, 1200),
                        (0, 6, -9, 1800),
                        (1, -12, 5, 2500),
                        (2, 3, 3, 1500),
                        (2, -25, 40, 2700),
                        (3, 8, 8, 1000),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        let player = [(centre + dx * 512) as f32, (centre + dz * 512) as f32];
                        let camera_z = if camera_state == 0 { -1 } else { 0 };
                        let input = RoofInput {
                            cycle: 200 + n as i32,
                            level: level.min(2),
                            camera_state,
                            camera: [
                                centre + dx * 512 + 1024,
                                eye_over_centre(built, 1500),
                                centre + dz * 512 + 2048,
                            ],
                            pitch,
                            player,
                            server: [
                                if camera_z == -1 {
                                    -1
                                } else {
                                    centre + dx * 512
                                },
                                centre + dz * 512,
                            ],
                            base: [built.base_x, built.base_z],
                            cam2_look: player,
                            cam2_eye: [player[0] + 2048.0, player[1] + 2048.0],
                        };
                        state.update(&world, &input);
                        d.put(&state.stamps);
                        d.put(&state.boxes);
                        d.put(&state.hide_roof);
                        d.put(&state.invalid_ray);
                        d.put(&state.draw_stamp(input.cycle));
                    }
                }
            }
            let _ = scratch;
        });
        out.list.last_mut().unwrap().count = marked as u64;
    }

    // The minimap: the queued map elements and locs, and the base plan of
    // every level.
    {
        let graph = built.scene_graph.as_ref().expect("scene graph");
        let mut minimap = Minimap::default();
        minimap.install(&stores.pack);
        minimap
            .locs
            .as_ref()
            .expect("minimap locs")
            .allow_members
            .set(true);
        let base = [built.base_x, built.base_z];
        minimap.ensure_world_map(base, size as i32);
        let mut marks = 0;
        out.add("minimap", 0, |d, scratch| {
            for level in 0..4 {
                minimap.queue_map_elements(base, size as i32, size as i32, level);
                d.put(&minimap.queued_elements);
                let ready = minimap.are_loc_icons_ready(graph, terrain, level, None);
                d.put(&ready);
                minimap.refresh(graph, terrain, level, None, &Default::default());
                d.put(&minimap.queued_locs);
                let plan = minimap.base_plan(graph, terrain, level, None);
                d.put(&plan.size);
                d.put(&plan.first_level);
                for (l, tiles) in &plan.level_tiles {
                    d.put(l);
                    d.put(tiles);
                }
                d.put(&plan.marks.len());
                marks += plan.marks.len();
                for mark in &plan.marks {
                    match mark {
                        Mark::Fill { rect, colour } => {
                            d.put(&0_u8);
                            d.put(rect);
                            d.put(colour);
                        }
                        Mark::Line { from, to, colour } => {
                            d.put(&1_u8);
                            d.put(from);
                            d.put(to);
                            d.put(colour);
                        }
                        Mark::Icon { rect, .. } => {
                            d.put(&2_u8);
                            d.put(rect);
                        }
                        Mark::Add { rect, colour } => {
                            d.put(&3_u8);
                            d.put(rect);
                            d.put(colour);
                        }
                    }
                }
            }
            let _ = scratch;
        });
        out.list.last_mut().unwrap().count = marks as u64;
    }

    // The draw planner over the cameras (with the roof stamps of the fixture
    // rows), including the depth raster and the submitted models, floor
    // batches, lights and shadows.
    let frames = cameras(built);
    let camera_count = frames.len();
    let traces = frame_traces(built, frames, materials).expect("frame traces");
    out.add("draw", camera_count, |d, _| {
        feed_trace(d, &traces.frames);
        feed_trace(d, &traces.submissions);
    });
    out.add("draw/materials", camera_count, |d, _| {
        feed_trace(d, &traces.materials);
    });

    // The underwater lists: only a region with an underwater scene has any.
    let uw_entities: Vec<DrawEntity> = built
        .underwater_models
        .iter()
        .map(|m| m.entity.clone())
        .collect();
    let uw_heights: Vec<_> = built
        .scene
        .underwater
        .iter()
        .flatten()
        .map(|g| g.heights.clone())
        .collect();
    let graph = built.scene_graph.as_ref().expect("scene graph");
    let mut occlusion = crate::occlusion::Occlusion::new(graph, &heights);
    let mut state = DrawState::new(32);
    let camera = cameras(built)[0];
    state.live_frame(
        PlannerScene {
            scene: graph,
            heights: &heights,
            entities: &[],
        },
        camera,
        None,
        &mut occlusion,
        LiveInputs {
            boxes: &[],
            cam2: None,
            underwater: Some((&uw_entities, &uw_heights)),
        },
    );
    out.add(
        "draw/underwater",
        state.plan.underwater_opaque.len() + state.plan.underwater_transparent.len(),
        |d, _| {
            d.put(
                &state
                    .plan
                    .underwater_opaque
                    .iter()
                    .map(|&i| i as u32)
                    .collect::<Vec<_>>(),
            );
            d.put(
                &state
                    .plan
                    .underwater_transparent
                    .iter()
                    .map(|&i| i as u32)
                    .collect::<Vec<_>>(),
            );
        },
    );
    out.list
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

fn fixture_path() -> PathBuf {
    rs910_core::test_support::client_dir().join("crates/rs910-scene/fixtures/scene-corpus.tsv")
}

/// `region<TAB>stage<TAB>count<TAB>digest` lines.
fn table(region: &str, stages: &[Stage]) -> Vec<String> {
    stages
        .iter()
        .map(|s| format!("{region}\t{}\t{}\t{:016x}", s.name, s.count, s.digest))
        .collect()
}

fn read_fixture() -> BTreeMap<(String, String), String> {
    let text = std::fs::read_to_string(fixture_path()).unwrap_or_default();
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
        .map(|l| {
            let mut cols = l.splitn(3, '\t');
            let region = cols.next().unwrap().to_string();
            let stage = cols.next().unwrap().to_string();
            ((region, stage), l.to_string())
        })
        .collect()
}

/// Squares the window covers, present in the cache or not.
fn expected_squares(built: &Rebuild) -> usize {
    let half = (built.map_size >> 4) as i32;
    let region = ((built.base_x >> 3) + half, (built.base_z >> 3) + half);
    window_squares(region.0, region.1, built.map_size).len()
}

fn selected() -> Vec<Region> {
    let filter = std::env::var("RS910_CORPUS_REGIONS").ok();
    regions()
        .into_iter()
        .filter(|r| {
            filter
                .as_ref()
                .is_none_or(|f| f.split(',').any(|name| name == r.name))
        })
        .collect()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn regions_match_the_committed_digest() {
    let pack = crate::test_support::require_pack("client.mapsv2.js5");
    let update = std::env::var_os("RS910_UPDATE_CORPUS").is_some();
    let wanted = selected();
    let mut now: Vec<(String, Vec<String>)> = Vec::new();
    // One thread per region: each loads its own stores (`LocStore` is not
    // `Sync`) and builds independently.
    std::thread::scope(|scope| {
        let handles: Vec<_> = wanted
            .iter()
            .map(|region| {
                let pack = pack.clone();
                scope.spawn(move || {
                    let started = std::time::Instant::now();
                    let stores = Stores::load(&pack);
                    let (mut built, terrain) = build(&stores, region);
                    let stages = stages(&stores, &mut built, &terrain);
                    eprintln!(
                        "corpus {}: {} stages, {} of {} squares present, {:.1}s",
                        region.name,
                        stages.len(),
                        built.squares.len(),
                        expected_squares(&built),
                        started.elapsed().as_secs_f32()
                    );
                    (region.name.to_string(), table(region.name, &stages))
                })
            })
            .collect();
        for handle in handles {
            now.push(handle.join().expect("region thread"));
        }
    });
    if update {
        let mut rows = read_fixture();
        for (region, lines) in &now {
            rows.retain(|(r, _), _| r != region);
            for line in lines {
                let stage = line.split('\t').nth(1).unwrap().to_string();
                rows.insert((region.clone(), stage), line.clone());
            }
        }
        let mut text = String::from(
            "# Scene corpus digest: see src/corpus.rs. Columns: region, stage, count, digest. Hashes and counts only.\n",
        );
        for region in regions() {
            for ((r, _), line) in &rows {
                if r == region.name {
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
    for (region, lines) in &now {
        for line in lines {
            let stage = line.split('\t').nth(1).unwrap();
            match expected.get(&(region.clone(), stage.to_string())) {
                Some(want) if want == line => {}
                Some(want) => bad.push(format!("{region}/{stage}: now `{line}`, fixture `{want}`")),
                None => bad.push(format!("{region}/{stage}: not in the fixture")),
            }
        }
        for (r, stage) in expected.keys() {
            if r == region && !lines.iter().any(|l| l.split('\t').nth(1) == Some(stage)) {
                bad.push(format!("{region}/{stage}: in the fixture, not produced"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "the scene layer produces different values ({} stages):\n{}",
        bad.len(),
        bad.join("\n")
    );
}
