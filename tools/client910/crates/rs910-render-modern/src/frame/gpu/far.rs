//! The far scene's GPU half (`docs/renderer/modern-renderer.md`;
//! the CPU halves
//! are the `rs910_far_scene` crate and [`crate::far`]).
//!
//! The modern client draws map squares from the cache in rings around the
//! camera focus out to one of five draw distances, not the server's 104-tile
//! build area. With `CLIENT910_MODERN_FAR=0..4` (default 2; `off` is the
//! classic window only) this backend:
//!
//! - **F0, projection and fog** (`ModernRenderer::far_uniforms`): draws with
//!   the modern far plane (`distance + 1024`) in place of the classic
//!   14,844 (never nearer) and fogs with the modern law, the fog ending at that far plane (the
//!   fog function is the existing one, only its range moves). Depth stays
//!   `Depth24Plus` with standard z: its resolution at view depth `z` is
//!   about `z^2 / (near 2^24)`, set by the near plane (200), 1.3 units at
//!   66,560, so the near scene's depth precision is unchanged.
//! - **F1, far terrain**: the map squares of the ring around the focus (the
//!   camera target), each built by the near terrain's builder from quiet
//!   map reads (`crate::terrain::build_square`) into the far texture array;
//!   the tiles inside the classic window are not drawn (the near terrain owns
//!   them), so near and far meet on the same grid with the same heights.
//!   Every level draws: the upper levels are the buildings' floors, which
//!   stand on the far locs.
//! - **F3, far locs**: each square's static locs outside the window,
//!   placed by the classic placement on a private scene
//!   (`rs910_far_scene::far_locs`), built as RT7 geometry with their LOD
//!   lists and merged per 16 x 16-tile container ([`crate::far::jobs`]).
//!   A container draws one draw per batch and run of models at one LOD
//!   ([`crate::far::batch`]).
//! - **F4, distance classes and LODs**: a square's locs are built while the
//!   square's class is within the level; a container draws while its class is
//!   (class 6 unloads it), and only the loc categories at or above its
//!   class; each model draws the LOD the model LOD rule gives it
//!   (`rs910_far_scene::far_level::model_lod` with the table's entry 0, its
//!   nearest box depth, its radius and its projected size). The modern
//!   client has no LOD cross-fade (its LOD table carries no blend factor),
//!   and the streaming fade-in (300 ms, `-1 -> 0`) is not drawn: the mapping
//!   of that value onto the shaders' fade input is not known.
//! - **F5, streaming**: squares start nearest first (at most
//!   [`FRAME_BUDGET`] a frame) and build on worker threads
//!   (`rs910_far_scene::far_jobs`): terrain on a small pool, locs on
//!   [`LOC_WORKERS`] workers (each holds its own loc configs); results are
//!   uploaded within [`UPLOAD_BUDGET`] bytes a frame, nearest first. The
//!   render thread never builds a square outside sync mode
//!   (`CLIENT910_MODERN_FAR_SYNC=1`, and the tests), which builds and
//!   uploads the whole ring before the frame so frames repeat.
//! - **F2, near extension**: the classic window's static content beyond the
//!   classic draw radius, under the plan's roof cut (`LiveScene::roof_removal`'s
//!   stamps): the near terrain's tiles the plan does not select, and the
//!   scene graph's static locs the plan left out outside its draw square
//!   or past the classic far plane, frustum-culled against the modern projection,
//!   without the sub-pixel ones (`MIN_PIXELS`). The opaque ones draw from
//!   merged 16 x 16-tile containers of the window's static locs (built on
//!   the loc workers from slim copies of their snapshot models, LOD 0 as
//!   the near path draws them): one draw per batch and run of selected locs
//!   instead of one per loc and material (performance plan bottleneck #8).
//!   A loc its container does not hold (its RT7 does not correspond, or it
//!   changed since) and the transparent ones draw through the per-loc path.
//!   Dynamic locs, NPCs and players stay plan-only.
//!
//! The forward and depth prepass draw the far scene; merged loc containers
//! are omitted from AO geometry. The near
//! extension's containers also draw into the water reflection (as its
//! per-loc draws did). Nothing of it casts sun shadows. The far scene is off
//! in instances (the near terrain's own test: a scene whose ground is not
//! its squares' map file 5) and underground (squares from row 100), and
//! while the modern terrain is off.
//!
//! `CLIENT910_MODERN_CHECK` adds: no extension loc is in the plan, no
//! extension tile is selected by the plan, no far tile lies inside the
//! window, and every far container was built against the current window.

use crate::frame::encoding::EncodeInputs;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::Arc;

use rs910_far_scene::far_jobs::{default_threads, FarJobs};
use rs910_far_scene::far_level::{self, FarLevel, MODEL_LOD};
use rs910_far_scene::far_locs::{container_of, CONTAINER_TILES};
use rs910_far_scene::far_ring::{self, SquareId, TileRect};
use rs910_far_scene::far_world::{FarView, FarWorld, FRAME_BUDGET};
use wgpu::util::DeviceExt;

use crate::draw::FloorSelection;
use crate::far::batch::{runs, DrawRun, MergedMesh};
use crate::far::jobs::{self, ExtBuild, ExtLoc, FarAssets, LocWorker, LocsBuild, TerrainBuild};
use crate::far::lod::ClassicMesh;
use crate::frame::arenas::{Alloc, LocArena};
use crate::frame::gpu::terrain::TerrainPass;
use crate::frame::*;
use crate::scene::EntityRef;
use crate::terrain::{LevelMesh, TerrainVertex};

/// Levels of a far square that draw: all four (the upper ones are the
/// buildings' floors, which stand on the far locs).
pub(crate) const DRAWN_LEVELS: usize = 4;

/// An extension loc inside the plan's draw square joins only when its view
/// depth is beyond the classic far plane minus this (fine units): inside the
/// classic reach the plan's own culling (occlusion) stands, which would only
/// add hidden draws. (Tiles join whenever the plan leaves them out: they cost
/// one draw per level, and the classic tile visibility, which assumes the player
/// camera's distance, drops visible tiles under a raised camera.)
pub(crate) const CLASSIC_FAR_MARGIN: f32 = 1024.0;

/// Extension locs smaller than this on screen (pixels, their box's
/// projection) are not drawn: distant ground decorations are sub-pixel.
pub(crate) const MIN_PIXELS: f32 = 1.5;

/// New per-loc extension meshes (RT7 builds) per frame outside sync mode;
/// the rest wait for the next frames.
pub(crate) const NEW_MESHES_PER_FRAME: usize = 64;

/// GPU bytes of far results uploaded per frame outside sync mode (a
/// stand-in budget; design §6.2 estimated at most 4 ms or 16 MB). The
/// nearest result always uploads, so a large square is never starved.
pub(crate) const UPLOAD_BUDGET: u64 = 8 << 20;

/// The render thread's time for the far scene's per-frame work outside
/// sync mode (uploads, placing squares at the scene's base, new materials,
/// the near extension's model copies; a stand-in budget, design §6.2's
/// 4 ms): each of them stops when it is spent, after its first item, and
/// resumes next frame.
pub(crate) const FRAME_TIME_BUDGET: std::time::Duration = std::time::Duration::from_millis(4);

/// Loc build workers (each holds its own copy of the loc configs).
pub(crate) const LOC_WORKERS: usize = 2;
/// The most far-terrain workers ([`default_threads`] of the cores left).
pub(crate) const TERRAIN_WORKERS: usize = 3;

/// The far scene's streaming workers on this machine (terrain and locs; the
/// frame's own pool leaves them their cores, `frame::jobs`).
#[must_use]
pub(crate) fn far_workers() -> usize {
    default_threads(TERRAIN_WORKERS) + LOC_WORKERS
}

/// Tiles around the classic window whose squares rebuild when it moves: a
/// loc of a square this close can be admitted by the window (its
/// footprint reaches in) and a terrain tile takes its boundary normals.
pub(crate) const WINDOW_MARGIN: i32 = 16;

/// One far square's CPU terrain (square-local, `crate::terrain::build_square`,
/// its vertex slots numbering the far texture array).
pub(crate) struct FarMesh {
    pub(crate) levels: Vec<Option<LevelMesh>>,
}

/// One far square on the GPU.
pub(crate) struct SquareGpu {
    /// Per drawn level: vertices (scene-local at `base`), indices over the
    /// tiles outside `window`, index count.
    pub(crate) levels: Vec<Option<(wgpu::Buffer, wgpu::Buffer, u32)>>,
    pub(crate) base: [i32; 2],
    pub(crate) window: TileRect,
    /// Scene-local bounds, classic y.
    pub(crate) bounds: [glam::Vec3; 2],
    pub(crate) bytes: u64,
}

/// The far terrain's texture-array layers (materials by first use, their
/// texels decoded by the terrain workers).
#[derive(Default)]
pub(crate) struct Layers {
    pub(crate) materials: Vec<i32>,
    pub(crate) index: HashMap<i32, u16>,
    pub(crate) texels: Vec<crate::terrain::atlas::LayerLevels>,
    pub(crate) params: Vec<[f32; 4]>,
    /// Layers added since the array's last upload.
    pub(crate) dirty: bool,
}

/// What a level's extension tiles were computed from.
#[derive(Clone, PartialEq)]
pub(crate) struct ExtKey {
    pub(crate) selection: FloorSelection,
    pub(crate) roof: Option<(i8, usize)>,
    pub(crate) far: i32,
}

/// One level's near-extension tiles (indices into the near terrain's
/// vertices of that level).
pub(crate) struct ExtLevel {
    pub(crate) key: ExtKey,
    pub(crate) indices: Option<wgpu::Buffer>,
    pub(crate) count: u32,
    pub(crate) tiles: Vec<usize>,
}

/// A merged mesh in the far scene's arena: its chunks' layouts (batches,
/// models, bounds; the geometry dropped once stored) and where each lives
/// (a loc page of [`FarGpu::arena`], `frame::arenas`).
pub(crate) struct MeshChunks {
    pub(crate) chunks: Vec<(MergedMesh, Alloc)>,
    pub(crate) bytes: u64,
}

impl MeshChunks {
    /// Store `meshes` in `arena` (`None`: nothing to draw).
    fn store(
        arena: &mut LocArena,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        meshes: Vec<MergedMesh>,
    ) -> Option<Self> {
        let mut chunks = Vec::with_capacity(meshes.len());
        let mut bytes = 0;
        for mut mesh in meshes {
            if mesh.indices.is_empty() {
                continue;
            }
            bytes += mesh.bytes();
            let streams = mesh.take_streams();
            chunks.push((mesh, arena.store(device, queue, None, &streams)));
        }
        (!chunks.is_empty()).then_some(Self { chunks, bytes })
    }

    /// Give its ranges back (from the next frame on).
    fn free(&self, arena: &mut LocArena) {
        for (_, a) in &self.chunks {
            arena.free(*a);
        }
    }
}

/// A far loc container on the GPU.
pub(crate) struct ContainerGpu {
    /// The square whose locs it holds and the window they were placed
    /// against.
    pub(crate) square: SquareId,
    pub(crate) window: TileRect,
    /// The smallest class it holds the categories of (a nearer class
    /// rebuilds the square's locs).
    pub(crate) class: u8,
    pub(crate) gpu: MeshChunks,
}

/// A started square's state.
#[derive(Default)]
pub(crate) struct SquareState {
    /// The window its builds exclude.
    pub(crate) window: TileRect,
    /// The CPU terrain (kept: the GPU copy is re-placed when the scene
    /// base moves).
    pub(crate) terrain: Option<FarMesh>,
    pub(crate) terrain_done: bool,
    pub(crate) locs_done: bool,
    /// The current loc build (a rebuild supersedes an outstanding one) and
    /// the level it was built for.
    pub(crate) loc_ticket: u64,
    pub(crate) loc_level: Option<FarLevel>,
}

/// The loc workers' job keys and results.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LocKey {
    Square(SquareId, u64),
    Extension((i32, i32), u64),
}

pub(crate) enum LocResult {
    Square(LocsBuild),
    Extension(ExtBuild),
}

/// A finished build waiting for its upload.
pub(crate) enum Ready {
    Terrain(SquareId, u64, TerrainBuild),
    Locs(SquareId, u64, LocsBuild),
    Extension((i32, i32), u64, ExtBuild),
}

impl Ready {
    /// The GPU bytes its upload creates.
    fn bytes(&self) -> u64 {
        let mesh = |m: &MergedMesh| {
            (m.vertices.len() * std::mem::size_of::<Vertex>()
                + m.colours.len() * 4
                + m.indices.len() * 4) as u64
        };
        match self {
            Self::Terrain(_, _, t) => t
                .levels
                .iter()
                .flatten()
                .map(|l| {
                    (l.vertices.len() * std::mem::size_of::<TerrainVertex>() + l.indices.len() * 4)
                        as u64
                })
                .sum(),
            Self::Locs(_, _, l) => l.containers.iter().flat_map(|c| &c.mesh).map(mesh).sum(),
            Self::Extension(_, _, e) => e.mesh.iter().map(mesh).sum(),
        }
    }
}

/// One near-extension container of the window's static locs.
pub(crate) enum ExtContainer {
    /// Its locs are being copied (over frames, within the frame budget):
    /// the next of its entity ids, the copies and their keys so far.
    Gathering {
        next: usize,
        locs: Vec<ExtLoc>,
        keys: HashMap<u32, EntityKey>,
    },
    /// Its build is queued or running: its generation and the key each loc
    /// was copied with.
    Queued {
        generation: u64,
        keys: HashMap<u32, EntityKey>,
    },
    Ready {
        gpu: Option<MeshChunks>,
        /// The key each merged loc was built for.
        keys: HashMap<u32, EntityKey>,
        /// A loc changed since: rebuild.
        stale: bool,
    },
}

/// The near extension's containers of one installed window.
#[derive(Default)]
pub(crate) struct ExtScene {
    /// The scene they belong to (its terrain key and static slot count).
    pub(crate) key: Option<(crate::terrain::SceneKey, usize)>,
    /// The window's static opaque locs by container (entity ids).
    pub(crate) by_container: BTreeMap<(i32, i32), Vec<usize>>,
    pub(crate) containers: HashMap<(i32, i32), ExtContainer>,
    pub(crate) generation: u64,
}

/// One batch a frame draws: where its chunk lives, its material,
/// camera-local origin, light slots, transparency, view depth and draw
/// ranges (in the chunk's indices).
pub(crate) struct BatchRecord {
    pub(crate) alloc: Alloc,
    pub(crate) material: i32,
    pub(crate) at: [f32; 3],
    pub(crate) lights: [f32; 4],
    pub(crate) transparent: bool,
    pub(crate) depth: f32,
    /// `(start, len)` of its draw ranges in [`FarGpu::range_scratch`].
    pub(crate) ranges: (usize, usize),
}

/// The far scene's counters (the log, the tests).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FarStats {
    /// The level drawing this frame (`None`: off).
    pub level: Option<u8>,
    /// Resident squares with terrain, and those drawn this frame.
    pub squares: usize,
    pub drawn_squares: usize,
    /// Far terrain triangles drawn this frame.
    pub far_triangles: usize,
    /// Far loc containers resident and drawn, the models they drew, and
    /// their draw calls (per pass).
    pub containers: usize,
    pub drawn_containers: usize,
    pub far_locs: usize,
    pub far_batches: usize,
    pub far_draws: usize,
    /// Compatible multi-draw runs and their original packets (forward list).
    pub indirect_runs: usize,
    pub indirect_packets: usize,
    /// The far loc containers' GPU bytes (part of `gpu_bytes`).
    pub far_bytes: u64,
    /// Near-extension tiles and locs this frame; of the locs, those drawn
    /// from containers, and the containers' draw calls (per pass).
    pub ext_tiles: usize,
    pub ext_locs: usize,
    pub ext_batched: usize,
    pub ext_transparent: usize,
    pub ext_draws: usize,
    /// The near extension's containers resident and their GPU bytes.
    pub ext_containers: usize,
    pub ext_bytes: u64,
    /// Far GPU bytes (terrain, layers, containers).
    pub gpu_bytes: u64,
    /// Render-thread time of this frame's far work outside the encode
    /// (planning, sync builds, uploads, culling), ms, and its parts:
    /// streaming and uploads, the terrain (layers, placement, culling), the
    /// far containers' draws (with new materials), the near extension.
    pub build_ms: f64,
    pub phases_ms: [f64; 4],
    /// Results uploaded this frame and their bytes.
    pub uploaded: usize,
    pub upload_bytes: u64,
    /// Builds queued or running on the workers, and results waiting for
    /// their upload.
    pub pending: usize,
    pub waiting: usize,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct FarGpu {
    /// Build the whole ring before each frame (deterministic frames; the
    /// tests set it).
    pub(crate) sync: bool,
    pub(crate) world: FarWorld<SquareState>,
    /// The classic window the squares were built against.
    pub(crate) window: TileRect,
    /// What the builds read (loaded on a thread of its own on first use),
    /// and the workers.
    pub(crate) assets: Option<Arc<FarAssets>>,
    pub(crate) assets_loading: Option<std::thread::JoinHandle<Option<FarAssets>>>,
    pub(crate) terrain_jobs: Option<FarJobs<(SquareId, u64), (), TerrainBuild>>,
    pub(crate) loc_jobs: Option<FarJobs<LocKey, LocWorker, LocResult>>,
    /// Finished builds waiting for their upload, nearest first.
    pub(crate) ready: VecDeque<Ready>,
    /// Loc builds started so far (their tickets).
    pub(crate) loc_tickets: u64,
    pub(crate) layers: Layers,
    pub(crate) layer_bytes: u64,
    pub(crate) bind: Option<wgpu::BindGroup>,
    pub(crate) squares: HashMap<SquareId, SquareGpu>,
    /// The far loc containers.
    pub(crate) containers: BTreeMap<(i32, i32), ContainerGpu>,
    /// Materials loaded for far batches this frame (their textures are read
    /// and uploaded on this thread, as the near path's are).
    pub(crate) new_materials: usize,
    /// This frame's end of [`FRAME_TIME_BUDGET`] (`None`: sync mode).
    pub(crate) deadline: Option<std::time::Instant>,
    /// The far layers' texture array: its capacity in layers and the layers
    /// written (grown by powers of two; new layers written in place).
    pub(crate) layer_array: Option<(wgpu::Texture, wgpu::TextureView, u32, usize)>,
    /// This frame's far terrain draws: `(square, level)`.
    pub(crate) draws: Vec<(SquareId, usize)>,
    /// The near extension: per level its tiles, this frame's levels with
    /// tiles, its per-loc locs (`live.entities` ids), its containers.
    pub(crate) ext: Vec<Option<ExtLevel>>,
    pub(crate) ext_draws: Vec<usize>,
    pub(crate) ext_opaque: Vec<usize>,
    pub(crate) ext_transparent: Vec<usize>,
    pub(crate) ext_scene: ExtScene,
    /// The entities drawn from extension containers this frame.
    pub(crate) ext_selected: Vec<bool>,
    /// The merged meshes' geometry: the far scene's own loc pages (its
    /// containers outlive the classic scene's installs, which reset the loc
    /// meshes' arena).
    pub(crate) arena: LocArena,
    /// This frame's merged-mesh draw packets (opaque first, then the far
    /// transparent ones from far to near, each with its camera-local
    /// matrix), joined to the frame's draw list where the near extension's
    /// per-loc draws go (`prepare_far_locs`).
    pub(crate) batch_draws: Vec<(Draw, [f32; 16], crate::models::bounds::Bounds)>,
    pub(crate) batch_opaque: usize,
    pub(crate) indirect: Option<(wgpu::Buffer, u64)>,
    pub(crate) indirect_args: Vec<wgpu::util::DrawIndexedIndirectArgs>,
    /// Scratch kept for its capacity: the per-model levels, a batch's runs,
    /// the frame's batch records and their ranges, the extension's
    /// candidates, wanted containers, container order and plan marks.
    pub(crate) lod_scratch: Vec<Option<u8>>,
    pub(crate) run_scratch: Vec<DrawRun>,
    pub(crate) record_scratch: Vec<BatchRecord>,
    pub(crate) range_scratch: Vec<DrawRun>,
    pub(crate) candidate_scratch: Vec<(usize, (i32, i32))>,
    pub(crate) wanted_scratch: Vec<(i32, i32)>,
    pub(crate) id_scratch: Vec<(i32, i32)>,
    pub(crate) planned_scratch: Vec<bool>,
    /// Whether the far scene draws this frame (decided before the frame
    /// uniforms, from the last scene's terrain).
    pub(crate) active: bool,
    pub(crate) level: Option<FarLevel>,
    /// The installed scene's key and whether its NXT terrain is usable (its
    /// ground is its squares' file 5: not an instance).
    pub(crate) scene: Option<(crate::terrain::SceneKey, bool)>,
    /// This frame's camera-local view, projection (NXT far plane, GL depth,
    /// column-major) and view-projection.
    pub(crate) view: glam::Mat4,
    pub(crate) projection: [f32; 16],
    pub(crate) view_proj: glam::Mat4,
    /// This frame's scene viewport, pixels.
    pub(crate) viewport: [f32; 2],
    pub(crate) stats: FarStats,
    pub(crate) frames: u64,
    pub(crate) mismatches: u64,
    /// Tests: draw the near extension per loc (the frame before batching).
    #[cfg(test)]
    pub(crate) test_per_loc: bool,
}

impl FarGpu {
    /// Whether this frame's render-thread budget is spent (never in sync
    /// mode).
    fn over_budget(&self) -> bool {
        self.deadline
            .is_some_and(|d| std::time::Instant::now() >= d)
    }

    /// Drop the far loc containers `drop` names, their arena ranges freed.
    fn drop_containers(&mut self, drop: impl Fn(&ContainerGpu) -> bool) {
        let arena = &mut self.arena;
        self.containers.retain(|_, c| {
            let gone = drop(c);
            if gone {
                c.gpu.free(arena);
            }
            !gone
        });
    }

    /// Drop the near extension's containers, their arena ranges freed.
    fn drop_extension(&mut self) {
        for c in self.ext_scene.containers.values() {
            if let ExtContainer::Ready { gpu: Some(g), .. } = c {
                g.free(&mut self.arena);
            }
        }
        self.ext_scene.containers.clear();
    }
}

/// The box's size on screen in pixels (`viewport` in pixels; the largest
/// side of its corners' NDC bounds, unbounded when a corner is behind the
/// eye); `None` when it is outside the frustum.
pub(crate) fn box_pixels(
    m: &glam::Mat4,
    lo: glam::Vec3,
    hi: glam::Vec3,
    viewport: [f32; 2],
) -> Option<f32> {
    if !box_visible(m, lo, hi) {
        return None;
    }
    let (mut min, mut max) = (glam::Vec2::splat(f32::MAX), glam::Vec2::splat(f32::MIN));
    for i in 0..8 {
        let p = glam::Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        let c = *m * p.extend(1.0);
        if c.w <= 1.0 {
            return Some(f32::MAX);
        }
        let n = glam::Vec2::new(c.x, c.y) / c.w;
        min = min.min(n);
        max = max.max(n);
    }
    let size = (max - min) * 0.5 * glam::Vec2::from(viewport);
    Some(size.x.max(size.y))
}

/// Whether any box corner survives every clip plane (wgpu depth, 0..w).
pub(crate) fn box_visible(m: &glam::Mat4, lo: glam::Vec3, hi: glam::Vec3) -> bool {
    let corners: [glam::Vec4; 8] = std::array::from_fn(|i| {
        let p = glam::Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        *m * p.extend(1.0)
    });
    let outside = |f: &dyn Fn(glam::Vec4) -> bool| corners.iter().all(|&c| f(c));
    !(outside(&|c| c.x < -c.w)
        || outside(&|c| c.x > c.w)
        || outside(&|c| c.y < -c.w)
        || outside(&|c| c.y > c.w)
        || outside(&|c| c.z < 0.0)
        || outside(&|c| c.z > c.w))
}

/// F2: level `floor`'s extension over `tiles` of the near terrain's `mesh`
/// (its indices uploaded under their key).
pub(crate) fn extension(
    device: &wgpu::Device,
    floor: &crate::models::draw_list::FloorDraw<'_>,
    mesh: &crate::terrain::LevelMesh,
    roof: Option<(i8, usize)>,
    far: i32,
    tiles: Vec<usize>,
) -> ExtLevel {
    let mut indices = Vec::new();
    for &tile in &tiles {
        let (start, count) = mesh.tile_ranges[tile];
        indices.extend_from_slice(&mesh.indices[start as usize..(start + count) as usize]);
    }
    let buffer = (!indices.is_empty()).then(|| {
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern far extension indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        })
    });
    ExtLevel {
        key: ExtKey {
            selection: floor.selection.clone(),
            roof,
            far,
        },
        indices: buffer,
        count: indices.len() as u32,
        tiles,
    }
}

/// `rect` grown by `margin` tiles on every side.
fn grown(rect: TileRect, margin: i32) -> TileRect {
    TileRect {
        x0: rect.x0 - margin,
        z0: rect.z0 - margin,
        x1: rect.x1 + margin,
        z1: rect.z1 + margin,
    }
}

/// The 2D distance, fine units, from `focus` to the tile rectangle `r`.
fn distance_to_rect(focus: [i32; 2], r: &TileRect) -> i32 {
    let axis = |p: i32, lo: i32, hi: i32| {
        if p < lo {
            lo - p
        } else if p > hi {
            p - hi
        } else {
            0
        }
    };
    let dx = f64::from(axis(focus[0], r.x0 * 512, r.x1 * 512));
    let dz = f64::from(axis(focus[1], r.z0 * 512, r.z1 * 512));
    dx.hypot(dz) as i32
}

/// Container `id`'s tiles.
fn container_rect(id: (i32, i32)) -> TileRect {
    TileRect {
        x0: id.0 * CONTAINER_TILES,
        z0: id.1 * CONTAINER_TILES,
        x1: (id.0 + 1) * CONTAINER_TILES,
        z1: (id.1 + 1) * CONTAINER_TILES,
    }
}

impl ModernRenderer {
    /// The draw-distance level (`ModernSettings::far`).
    pub(crate) fn far_level(&self) -> Option<FarLevel> {
        self.preparation.settings.far
    }

    /// Whether the whole ring is built before the frame
    /// (`CLIENT910_MODERN_FAR_SYNC=1`, or a test's [`FarGpu::sync`]).
    pub(crate) fn far_sync(&self) -> bool {
        self.scene_resources.far.sync || rs910_far_scene::far_debug_flags::flags().sync
    }

    /// F0: this frame's far plane and fog in `uniforms` (see the module
    /// docs); decides whether the far scene draws this frame. Off, it
    /// leaves `uniforms` as they are.
    pub(crate) fn far_uniforms(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        viewport: (i32, i32),
        uniforms: &mut FrameUniforms,
    ) {
        let level = self.far_level();
        let target = snapshot.camera.target;
        let surface = far_ring::on_surface(far_ring::square_of([target[0], target[2]]));
        let usable = level.is_some()
            && surface
            && self
                .scene_resources
                .far
                .scene
                .as_ref()
                .is_some_and(|(key, ok)| *ok && *key == crate::terrain::SceneKey::of(snapshot));
        self.scene_resources.far.active = usable;
        self.scene_resources.far.level = level.filter(|_| usable);
        let Some(level) = self.scene_resources.far.level else {
            return;
        };
        let far = level.far_plane() as f32;
        let mut local = snapshot.camera.clone();
        local.viewport = viewport;
        local.target = [0; 3];
        let view = local.view_entries();
        let projection = far_level::projection_with_far(&local.projection(), far);
        let vp = crate::camera::multiply(&view, &projection);
        let view_proj = crate::camera::gl_to_wgpu_depth() * crate::camera::to_glam(&vp);
        uniforms.view_proj = view_proj.to_cols_array_2d();
        self.scene_resources.far.view_proj = view_proj;
        self.scene_resources.far.view = crate::camera::to_glam(&view);
        self.scene_resources.far.projection = projection;
        self.scene_resources.far.viewport = [viewport.0 as f32, viewport.1 as f32];
        if uniforms.fog_colour[3] > 0.0 {
            if let Some((start, end)) = far_level::fog_range(far, snapshot.env.fog.range) {
                uniforms.fog_range = [start, 1.0 / (end - start), 0.0, 0.0];
            }
        }
    }

    /// The shared build inputs and the workers: loaded on a thread of
    /// their own on first use (the far scene waits for them; sync mode
    /// waits here).
    fn ensure_far_assets(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        sync: bool,
    ) -> Option<Arc<FarAssets>> {
        if let Some(a) = self.scene_resources.far.assets.as_ref() {
            return Some(Arc::clone(a));
        }
        if self.scene_resources.far.assets_loading.is_none() {
            let (Some(pack), Some(materials)) = (snapshot.pack, snapshot.materials) else {
                return None;
            };
            let (pack, materials) = (pack.clone(), materials.clone());
            let loader = std::thread::Builder::new()
                .name("far-assets".into())
                .spawn(move || match rs910_config::flo::FloStore::load(&pack) {
                    Ok(flo) => Some(FarAssets::load(&pack, &materials, &flo)),
                    Err(err) => {
                        log::warn!("[modern] far scene: flo configs: {err:#}");
                        None
                    }
                })
                .expect("spawn the far assets loader");
            self.scene_resources.far.assets_loading = Some(loader);
        }
        if !sync
            && !self
                .scene_resources
                .far
                .assets_loading
                .as_ref()
                .is_some_and(|h| h.is_finished())
        {
            return None;
        }
        let assets = Arc::new(
            self.scene_resources
                .far
                .assets_loading
                .take()?
                .join()
                .ok()??,
        );
        let a = Arc::clone(&assets);
        self.scene_resources.far.terrain_jobs = Some(FarJobs::new(
            "far-terrain",
            default_threads(TERRAIN_WORKERS),
            || (),
        ));
        self.scene_resources.far.loc_jobs =
            Some(FarJobs::new("far-locs", LOC_WORKERS, move || {
                LocWorker::new(Arc::clone(&a))
            }));
        self.scene_resources.far.assets = Some(Arc::clone(&assets));
        Some(assets)
    }

    /// F1-F5: after the near terrain: the ring's squares (started, built,
    /// uploaded, culled), the far layers, the far loc containers and the
    /// near extension's tiles and locs.
    pub(crate) fn prepare_far(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        list: &DrawList<'_>,
        origin: [f32; 3],
    ) {
        let start = std::time::Instant::now();
        // The installed scene's usability, for the next frames' decision.
        if self.far_level().is_some() {
            let key = crate::terrain::SceneKey::of(snapshot);
            let ok = self
                .scene_resources
                .terrain
                .scene
                .as_ref()
                .is_some_and(|s| s.key == key && s.cpu.usable);
            if self.scene_resources.far.scene.as_ref().map(|(k, _)| k) != Some(&key) {
                self.scene_resources.far.ext.clear();
            }
            self.scene_resources.far.scene = Some((key, ok));
        }
        let sync = self.far_sync();
        let far = &mut self.scene_resources.far;
        far.draws.clear();
        far.ext_draws.clear();
        far.ext_opaque.clear();
        far.ext_transparent.clear();
        far.arena.begin_frame();
        far.deadline = (!sync).then(|| start + FRAME_TIME_BUDGET);
        far.batch_draws.clear();
        far.batch_opaque = 0;
        far.new_materials = 0;
        far.stats = FarStats::default();
        let Some(level) = far
            .level
            .filter(|_| far.active && self.scene_resources.terrain.active)
        else {
            far.active = false;
            return;
        };
        let (Some(_), Some(_)) = (snapshot.pack, snapshot.materials) else {
            far.active = false;
            return;
        };
        far.stats.level = Some(level.index());
        let Some(assets) = self.ensure_far_assets(snapshot, sync) else {
            self.scene_resources.far.active = false;
            return;
        };
        let window = snapshot
            .floors
            .first()
            .and_then(Option::as_ref)
            .map_or_else(TileRect::default, |g| {
                TileRect::at(snapshot.floor_base, [g.tiles_x, g.tiles_z])
            });
        let target = snapshot.camera.target;
        let view = FarView {
            focus: [target[0], target[2]],
            level,
        };
        let phase = std::time::Instant::now();
        self.far_stream(&assets, window, &view, sync);
        self.far_upload(device, queue, sync);
        self.scene_resources.far.stats.phases_ms[0] = phase.elapsed().as_secs_f64() * 1000.0;
        let phase = std::time::Instant::now();
        // The layers of the materials the new squares named.
        if self.scene_resources.far.layers.dirty || self.scene_resources.far.bind.is_none() {
            self.upload_far_layers(device, queue);
        }
        self.scene_resources.far.stats.phases_ms[1] = phase.elapsed().as_secs_f64() * 1000.0;
        // (Re-)place the squares' terrain at the scene's base.
        // (Within the frame's budget after the first: a square not placed
        // yet draws from a later frame.)
        let base = snapshot.floor_base;
        let far = &mut self.scene_resources.far;
        let mut placed = 0;
        for square in far.world.squares() {
            let Some(mesh) = square.value.terrain.as_ref() else {
                continue;
            };
            let stale = far
                .squares
                .get(&square.id)
                .is_none_or(|g| g.base != base || g.window != square.value.window);
            if !stale {
                continue;
            }
            if placed > 0 && far.deadline.is_some_and(|d| std::time::Instant::now() >= d) {
                far.squares.remove(&square.id);
                continue;
            }
            placed += 1;
            far.squares.insert(
                square.id,
                upload_square(device, square.id, mesh, base, square.value.window),
            );
        }
        // Cull the terrain.
        let o = glam::Vec3::from(origin);
        for (&id, gpu) in &far.squares {
            far.stats.squares += 1;
            far.stats.gpu_bytes += gpu.bytes;
            if !box_visible(&far.view_proj, gpu.bounds[0] - o, gpu.bounds[1] - o) {
                continue;
            }
            let mut drawn = false;
            for (lvl, l) in gpu.levels.iter().enumerate() {
                if let Some((_, _, count)) = l {
                    far.draws.push((id, lvl));
                    far.stats.far_triangles += *count as usize / 3;
                    drawn = true;
                }
            }
            far.stats.drawn_squares += usize::from(drawn);
        }
        // Near squares first (early depth rejection), in a fixed order: the
        // draw order decides exact depth ties, so frames repeat.
        let focus = far_ring::square_of(view.focus);
        far.draws.sort_by_key(|&(sq, level)| {
            let (dx, dz) = (sq.0 - focus.0, sq.1 - focus.1);
            (dx * dx + dz * dz, sq, level)
        });
        far.stats.gpu_bytes += far.layer_bytes;
        // F3/F4: the far loc containers.
        let phase = std::time::Instant::now();
        self.prepare_far_containers(device, queue, snapshot, origin, &view);
        self.scene_resources.far.stats.phases_ms[2] = phase.elapsed().as_secs_f64() * 1000.0;
        let phase = std::time::Instant::now();
        // F2: the near extension.
        let prep = PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        };
        self.prepare_far_extension(&prep, list, level, sync);
        self.scene_resources.far.stats.phases_ms[3] = phase.elapsed().as_secs_f64() * 1000.0;
        self.scene_resources.far.frames += 1;
        self.scene_resources.far.stats.build_ms = start.elapsed().as_secs_f64() * 1000.0;
        self.scene_resources.far.stats.pending = self
            .scene_resources
            .far
            .terrain_jobs
            .as_ref()
            .map_or(0, FarJobs::pending)
            + self
                .scene_resources
                .far
                .loc_jobs
                .as_ref()
                .map_or(0, FarJobs::pending);
        self.scene_resources.far.stats.waiting = self.scene_resources.far.ready.len();
        self.check_far(snapshot, list);
        if self.scene_resources.far.frames == 1
            || self.scene_resources.far.frames.is_multiple_of(600)
        {
            let s = self.scene_resources.far.stats;
            log::info!(
                "[modern] far scene level {}: {} squares ({} drawn, {} triangles), {} loc containers ({} drawn, {} locs, {} draws), {:.1} MB GPU, {} layers; near extension {} tiles, {} locs ({} batched, {} draws); {} builds pending, {} uploads waiting",
                level.index(),
                s.squares,
                s.drawn_squares,
                s.far_triangles,
                s.containers,
                s.drawn_containers,
                s.far_locs,
                s.far_draws,
                s.gpu_bytes as f64 / 1e6,
                self.scene_resources.far.layers.materials.len(),
                s.ext_tiles,
                s.ext_locs,
                s.ext_batched,
                s.ext_draws,
                s.pending,
                s.waiting
            );
        }
    }

    /// F5: the ring's streaming: window moves and evictions drop what they
    /// invalidate, missing squares within the level start (nearest first,
    /// [`FRAME_BUDGET`] a frame; all of them, built here, in sync mode), and
    /// the workers' finished builds join the upload queue.
    fn far_stream(
        &mut self,
        assets: &Arc<FarAssets>,
        window: TileRect,
        view: &FarView,
        sync: bool,
    ) {
        let far = &mut self.scene_resources.far;
        // A moved window: the squares near it rebuild against the new one
        // (their GPU copies go now: they may overlap it).
        if far.window != window {
            let (old, new) = (
                grown(far.window, WINDOW_MARGIN),
                grown(window, WINDOW_MARGIN),
            );
            let touches = |sq: SquareId| {
                let r = TileRect::of_square(sq);
                r.intersects(&old) || r.intersects(&new)
            };
            far.world.invalidate(&touches);
            far.squares.retain(|&sq, _| !touches(sq));
            far.drop_containers(|c| touches(c.square));
            far.ready.retain(|r| match r {
                Ready::Terrain(sq, ..) | Ready::Locs(sq, ..) => !touches(*sq),
                Ready::Extension(..) => true,
            });
            far.window = window;
        }
        let plan = far.world.plan(view);
        if !plan.evicted.is_empty() {
            let evicted = |sq: &SquareId| plan.evicted.contains(sq);
            let evicted_key = |k: &LocKey| matches!(k, LocKey::Square(sq, _) if evicted(sq));
            far.squares.retain(|sq, _| !evicted(sq));
            far.drop_containers(|c| evicted(&c.square));
            far.ready.retain(|r| match r {
                Ready::Terrain(sq, ..) | Ready::Locs(sq, ..) => !evicted(sq),
                Ready::Extension(..) => true,
            });
            if let Some(jobs) = far.terrain_jobs.as_mut() {
                jobs.retain_queued(|(sq, _)| !evicted(sq));
            }
            if let Some(jobs) = far.loc_jobs.as_mut() {
                jobs.retain_queued(|k| !evicted_key(k));
            }
        }
        let centre = far_ring::square_of(view.focus);
        let radius = view.level.ring_radius();
        assets
            .terrain
            .retain(&|sq| far_ring::within(centre, sq, radius + 2));
        // Start the missing squares within the level (a square's locs are
        // built only while its class is within the level; its terrain
        // follows the same gate here).
        let mut started = 0;
        for sq in plan.missing {
            let class = far_level::distance_class(far_ring::distance_to_square(view.focus, sq));
            if class > view.level.index() || !far_ring::on_surface(sq) {
                continue;
            }
            if !sync && started >= FRAME_BUDGET {
                break;
            }
            started += 1;
            let ticket = far.world.start(
                sq,
                view,
                SquareState {
                    window,
                    ..SquareState::default()
                },
            );
            let Some(terrain_jobs) = far.terrain_jobs.as_mut() else {
                continue;
            };
            let priority = i64::from(far_ring::distance_to_square(view.focus, sq));
            let a = Arc::clone(assets);
            if sync {
                let t = terrain_jobs.run_inline(|_| jobs::terrain(&a, sq, window));
                far.ready.push_back(Ready::Terrain(sq, ticket, t));
            } else {
                terrain_jobs.submit((sq, ticket), priority, move |()| {
                    jobs::terrain(&a, sq, window)
                });
            }
            far_start_locs(far, sq, view, sync);
        }
        // Squares whose locs were built for another level rebuild them.
        let other_level: Vec<SquareId> = far
            .world
            .squares()
            .filter(|s| s.value.loc_level.is_some_and(|l| l != view.level))
            .map(|s| s.id)
            .collect();
        for sq in other_level {
            far_start_locs(far, sq, view, sync);
        }
        // The workers' finished builds (stale ones dropped).
        let mut arrived = Vec::new();
        if let Some(jobs) = far.terrain_jobs.as_mut() {
            for ((sq, ticket), t) in jobs.take_results() {
                arrived.push(Ready::Terrain(sq, ticket, t));
            }
        }
        if let Some(jobs) = far.loc_jobs.as_mut() {
            for (key, r) in jobs.take_results() {
                match (key, r) {
                    (LocKey::Square(sq, ticket), LocResult::Square(l)) => {
                        arrived.push(Ready::Locs(sq, ticket, l));
                    }
                    (LocKey::Extension(id, generation), LocResult::Extension(e)) => {
                        arrived.push(Ready::Extension(id, generation, e));
                    }
                    _ => {}
                }
            }
        }
        far.ready.extend(arrived);
        // Nearest first.
        let focus = view.focus;
        let distance = |r: &Ready| match r {
            Ready::Terrain(sq, ..) | Ready::Locs(sq, ..) => {
                far_ring::distance_to_square(focus, *sq)
            }
            Ready::Extension(id, ..) => distance_to_rect(focus, &container_rect(*id)),
        };
        far.ready.make_contiguous().sort_by_key(|r| distance(r));
    }

    /// F5: upload the finished builds, nearest first, within
    /// [`UPLOAD_BUDGET`] bytes (all of them in sync mode); stale results
    /// are dropped.
    fn far_upload(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        sync: bool,
    ) {
        let mut spent = 0_u64;
        while let Some(next) = self.scene_resources.far.ready.front() {
            let bytes = next.bytes();
            if !sync
                && spent > 0
                && (spent + bytes > UPLOAD_BUDGET || self.scene_resources.far.over_budget())
            {
                break;
            }
            let next = self
                .scene_resources
                .far
                .ready
                .pop_front()
                .expect("a ready build");
            spent += bytes;
            let far = &mut self.scene_resources.far;
            match next {
                Ready::Terrain(sq, ticket, t) => {
                    let Some(square) = far.world.current_mut(sq, ticket) else {
                        continue;
                    };
                    // The worker's local material numbers onto the far
                    // texture array's layers.
                    let layers = &mut far.layers;
                    let materials = far.assets.as_ref().map(|a| a.materials());
                    let slots: Vec<u16> = t
                        .materials
                        .iter()
                        .zip(&t.layers)
                        .map(|(&m, texels)| {
                            *layers.index.entry(m).or_insert_with(|| {
                                let material = u32::try_from(m)
                                    .ok()
                                    .and_then(|id| materials.and_then(|s| s.get(id)));
                                layers.materials.push(m);
                                layers
                                    .params
                                    .push(crate::terrain::layer_params(material, texels.is_some()));
                                layers.texels.push(
                                    texels.as_ref().map(|l| (**l).clone()).unwrap_or_default(),
                                );
                                layers.dirty = true;
                                (layers.materials.len() - 1) as u16
                            })
                        })
                        .collect();
                    let mut levels = t.levels;
                    for level in levels.iter_mut().flatten() {
                        for v in &mut level.vertices {
                            for s in &mut v.slots[..3] {
                                if let Some(&g) = slots.get(usize::from(*s)) {
                                    *s = g;
                                }
                            }
                        }
                    }
                    square.value.terrain_done = true;
                    square.value.terrain = levels
                        .iter()
                        .any(Option::is_some)
                        .then_some(FarMesh { levels });
                    far.squares.remove(&sq);
                }
                Ready::Locs(sq, ticket, l) => {
                    let Some(square) = far
                        .world
                        .square_mut(sq)
                        .filter(|s| s.value.loc_ticket == ticket)
                    else {
                        continue;
                    };
                    square.value.locs_done = true;
                    let window = square.value.window;
                    far.drop_containers(|c| c.square == sq);
                    for c in l.containers {
                        if let Some(gpu) = MeshChunks::store(&mut far.arena, device, queue, c.mesh)
                        {
                            far.containers.insert(
                                c.id,
                                ContainerGpu {
                                    square: sq,
                                    window,
                                    class: c.class,
                                    gpu,
                                },
                            );
                        }
                    }
                    if l.stats.face_mismatches > 0 {
                        log::warn!(
                            "[modern] far square {sq:?}: {} RT7 locs draw other faces than their classic models",
                            l.stats.face_mismatches
                        );
                    }
                }
                Ready::Extension(id, generation, e) => {
                    let Some(ExtContainer::Queued {
                        generation: g,
                        keys,
                    }) = far.ext_scene.containers.get_mut(&id)
                    else {
                        continue;
                    };
                    if *g != generation {
                        continue;
                    }
                    let keys = std::mem::take(keys);
                    let slots: std::collections::HashSet<u32> = e.slots.iter().copied().collect();
                    far.ext_scene.containers.insert(
                        id,
                        ExtContainer::Ready {
                            gpu: MeshChunks::store(&mut far.arena, device, queue, e.mesh),
                            keys: keys
                                .into_iter()
                                .filter(|(s, _)| slots.contains(s))
                                .collect(),
                            stale: false,
                        },
                    );
                }
            }
            self.scene_resources.far.stats.uploaded += 1;
            self.scene_resources.far.stats.upload_bytes += bytes;
        }
    }

    /// F3/F4: this frame's far loc container draws: each container within
    /// the level (its class), frustum-culled, draws its models of the
    /// categories its class keeps at their LODs, in runs.
    fn prepare_far_containers(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
        view: &FarView,
    ) {
        let base = snapshot.floor_base;
        let o = glam::Vec3::from(origin);
        let camera_local = |abs: [i32; 3]| {
            glam::Vec3::new(
                (abs[0] - base[0] * 512) as f32,
                abs[1] as f32,
                (abs[2] - base[1] * 512) as f32,
            ) - o
        };
        let z_row = self.scene_resources.far.view.row(2);
        let mut records = std::mem::take(&mut self.scene_resources.far.record_scratch);
        records.clear();
        let mut ranges = std::mem::take(&mut self.scene_resources.far.range_scratch);
        ranges.clear();
        let mut lods = std::mem::take(&mut self.scene_resources.far.lod_scratch);
        let mut run = std::mem::take(&mut self.scene_resources.far.run_scratch);
        let mut rebuild: Vec<SquareId> = Vec::new();
        let far = &mut self.scene_resources.far;
        for (&id, c) in &far.containers {
            far.stats.containers += 1;
            far.stats.gpu_bytes += c.gpu.bytes;
            far.stats.far_bytes += c.gpu.bytes;
            let Some(class) = far_level::container_class(
                distance_to_rect(view.focus, &container_rect(id)),
                view.level,
            ) else {
                continue;
            };
            if class < c.class && !rebuild.contains(&c.square) {
                // Nearer than its build reached: the categories it now
                // keeps are not in it.
                rebuild.push(c.square);
            }
            let mut drawn = 0;
            for (mesh, alloc) in &c.gpu.chunks {
                let at = camera_local(mesh.origin);
                let (lo, hi) = (
                    at + glam::Vec3::from(mesh.bounds[0]),
                    at + glam::Vec3::from(mesh.bounds[1]),
                );
                if !box_visible(&far.view_proj, lo, hi) {
                    continue;
                }
                // Each model's LOD (the model LOD rule, entry 0), or `None` when its
                // category is left out at this class.
                lods.clear();
                let mut drawn_models = 0;
                for m in &mesh.models {
                    if !far_level::category_draws(m.category, class) {
                        lods.push(None);
                        continue;
                    }
                    let centre = at + glam::Vec3::from(m.centre);
                    let c4 = centre.extend(1.0);
                    let depth_centre = z_row.dot(c4);
                    let reach = z_row.x.abs() * m.half[0]
                        + z_row.y.abs() * m.half[1]
                        + z_row.z.abs() * m.half[2];
                    let radius = m.radius();
                    let view_centre = (far.view * c4).truncate().to_array();
                    let lod =
                        far_level::model_lod(&MODEL_LOD[0], depth_centre - reach, radius, || {
                            far_level::projected_size(&far.projection, view_centre, radius)
                        });
                    lods.push(Some(lod));
                    drawn_models += 1;
                }
                if drawn_models == 0 {
                    continue;
                }
                drawn += drawn_models;
                let depth = z_row.dot(((lo + hi) * 0.5).extend(1.0));
                for batch in &mesh.batches {
                    runs(batch, &mesh.models, |m| lods[m as usize], &mut run);
                    if run.is_empty() {
                        continue;
                    }
                    records.push(BatchRecord {
                        alloc: *alloc,
                        material: batch.material,
                        at: at.to_array(),
                        lights: batch.lights,
                        transparent: batch.transparent,
                        depth,
                        ranges: (ranges.len(), run.len()),
                    });
                    ranges.extend_from_slice(&run);
                }
            }
            if drawn > 0 {
                far.stats.drawn_containers += 1;
                far.stats.far_locs += drawn;
            }
        }
        self.scene_resources.far.lod_scratch = lods;
        self.scene_resources.far.run_scratch = run;
        let sync = self.far_sync();
        for sq in rebuild {
            far_start_locs(&mut self.scene_resources.far, sq, view, sync);
        }
        if sync {
            self.far_upload(device, queue, true);
        }
        // Opaque first, by page and material (fewer binds); the
        // transparent ones far to near.
        records.sort_by(|a, b| {
            a.transparent.cmp(&b.transparent).then(if a.transparent {
                b.depth.total_cmp(&a.depth)
            } else {
                (a.alloc.page, a.material, a.alloc.vertex).cmp(&(
                    b.alloc.page,
                    b.material,
                    b.alloc.vertex,
                ))
            })
        });
        self.scene_resources.far.range_scratch = ranges;
        for r in records.drain(..) {
            let transparent = r.transparent;
            let draws = self.batch_draws(device, queue, snapshot, &r);
            if draws == 0 {
                continue;
            }
            self.scene_resources.far.stats.far_batches += 1;
            self.scene_resources.far.stats.far_draws += draws;
            if !transparent {
                self.scene_resources.far.batch_opaque = self.scene_resources.far.batch_draws.len();
            }
        }
        self.scene_resources.far.record_scratch = records;
    }

    /// Record `r`'s draw packets (see [`FarGpu::batch_draws`]); returns how
    /// many (none while its material cannot load).
    fn batch_draws(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        r: &BatchRecord,
    ) -> usize {
        let Some(instance) =
            self.batch_instance(device, queue, snapshot, r.material, r.at, r.lights)
        else {
            return 0;
        };
        let matrix = self.frame_resources.instances[instance as usize].model;
        let geometry = Geometry::Far {
            page: r.alloc.page,
            base_vertex: r.alloc.vertex as i32,
        };
        let first = r.alloc.first_index();
        let (at, len) = r.ranges;
        for k in at..at + len {
            let run = self.scene_resources.far.range_scratch[k];
            let origin = glam::Vec3::from(r.at);
            let bounds = crate::models::bounds::Bounds {
                min: (origin + glam::Vec3::from(run.bounds.min)).to_array(),
                max: (origin + glam::Vec3::from(run.bounds.max)).to_array(),
            };
            self.scene_resources.far.batch_draws.push((
                Draw {
                    geometry,
                    material: r.material,
                    first_index: first + run.first,
                    count: run.count,
                    instance,
                    pass: Pass::Opaque,
                    casts: false,
                    indirect: None,
                },
                matrix,
                bounds,
            ));
        }
        len
    }

    /// A batch instance at camera-local `at` for `material` (loaded if its
    /// textures are resident; `None` otherwise: nothing of it draws).
    fn batch_instance(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        material: i32,
        at: [f32; 3],
        lights: [f32; 4],
    ) -> Option<u32> {
        if self.device_resources.textures.get(material).is_none() {
            // A new material: its load (the texture reads and uploads) is
            // this thread's work, one or a few a frame; its residency the
            // loc workers checked already.
            if self.scene_resources.far.new_materials > 0 && self.scene_resources.far.over_budget()
            {
                return None;
            }
            let quiet = self
                .scene_resources
                .far
                .assets
                .as_ref()
                .is_some_and(|a| a.material_quiet(material));
            if !quiet {
                return None;
            }
            self.scene_resources.far.new_materials += 1;
            self.device_resources.textures.ensure(
                device,
                queue,
                snapshot.pack,
                snapshot.materials,
                material,
            );
        }
        let matrix = [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, at[0], at[1], at[2], 1.0,
        ];
        let mut record = self.instance(matrix, material, 1.0, 0);
        record.p2 = lights;
        self.frame_resources.instances.push(record);
        Some(self.frame_resources.instances.len() as u32 - 1)
    }

    /// The far layers' texture array, material table and bind group (the
    /// near terrain's layout, sampler and uniforms; the layers' texels come
    /// decoded from the terrain workers, [`crate::far::jobs::FarAssets::layer`]).
    pub(crate) fn upload_far_layers(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        let layers = &mut self.scene_resources.far.layers;
        layers.dirty = false;
        let (Some(pipes), Some(uniforms)) = (
            self.scene_resources.terrain.pipes.as_ref(),
            self.scene_resources.terrain.uniforms.as_ref(),
        ) else {
            return;
        };
        // The array grows by powers of two; new layers are written in
        // place (a new array writes them all).
        let count = layers.texels.len().max(1) as u32;
        let grow = self
            .scene_resources
            .far
            .layer_array
            .as_ref()
            .is_none_or(|(_, _, capacity, _)| *capacity < count);
        if grow {
            let capacity = count.next_power_of_two().max(16);
            let size = crate::terrain::atlas::LAYER_SIZE;
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("modern far terrain layers"),
                size: wgpu::Extent3d {
                    width: size,
                    height: size,
                    depth_or_array_layers: capacity,
                },
                mip_level_count: size.ilog2() + 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            for (layer, levels) in layers.texels.iter().enumerate() {
                write_layer(queue, &texture, layer as u32, levels);
            }
            let view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            });
            self.scene_resources.far.layer_array =
                Some((texture, view, capacity, layers.texels.len()));
        } else if let Some((texture, _, _, written)) = self.scene_resources.far.layer_array.as_mut()
        {
            for (layer, levels) in layers.texels.iter().enumerate().skip(*written) {
                write_layer(queue, texture, layer as u32, levels);
            }
            *written = layers.texels.len();
        }
        let Some((_, view, _, _)) = self.scene_resources.far.layer_array.as_ref() else {
            return;
        };
        let layers = &self.scene_resources.far.layers;
        let mut table = layers.params.clone();
        if table.is_empty() {
            table.push([0.0; 4]);
        }
        let materials_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern far terrain materials"),
            contents: bytemuck::cast_slice(&table),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 8,
                resource: uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::Sampler(&pipes.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: materials_buffer.as_entire_binding(),
            },
        ];
        if let (true, Some(b)) = (
            pipes.caustics,
            self.frame_resources.water.caustics.buffers.as_ref(),
        ) {
            entries.push(wgpu::BindGroupEntry {
                binding: 12,
                resource: b.uniforms.as_entire_binding(),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 13,
                resource: b.light.as_entire_binding(),
            });
        }
        self.scene_resources.far.bind =
            Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern far terrain"),
                layout: &pipes.layout,
                entries: &entries,
            }));
    }

    /// F2: the window's tiles and static locs the plan left out (see the
    /// module docs).
    pub(crate) fn prepare_far_extension(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        list: &DrawList<'_>,
        level: FarLevel,
        sync: bool,
    ) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
            return;
        };
        let far_plane = level.far_plane();
        let roof = live.roof_removal();
        let roof_key = roof.as_ref().map(|r| (r.stamp, r.first_plane));
        let stamp_at = |plane: usize, x: i32, z: i32| -> Option<i8> {
            roof.as_ref()?
                .stamps
                .get(plane)?
                .get(usize::try_from(x).ok()?)?
                .get(usize::try_from(z).ok()?)
                .copied()
        };
        let hidden_plane = |plane: usize, x: i32, z: i32| {
            roof.as_ref()
                .is_some_and(|r| plane >= r.first_plane && stamp_at(plane, x, z) == Some(r.stamp))
        };
        // Tiles, per level the near terrain covers.
        let Some(terrain) = self.scene_resources.terrain.scene.as_ref() else {
            return;
        };
        if self.scene_resources.far.ext.len() < terrain.levels.len() {
            self.scene_resources
                .far
                .ext
                .resize_with(terrain.levels.len(), || None);
        }
        for floor in &list.floors {
            let (Some(Some(mesh)), Some(Some(_))) = (
                terrain.cpu.levels.get(floor.level),
                terrain.levels.get(floor.level),
            ) else {
                continue;
            };
            // (Compared in place: the selection is cloned only for a new key.)
            let fresh = self.scene_resources.far.ext[floor.level]
                .as_ref()
                .is_none_or(|e| {
                    e.key.selection != *floor.selection
                        || e.key.roof != roof_key
                        || e.key.far != far_plane
                });
            if fresh {
                let [tx, tz] = mesh.tiles;
                let mut tiles = Vec::new();
                for z in 0..tz as i32 {
                    for x in 0..tx as i32 {
                        if floor.selection.visible(x, z) || !mesh.has_tile(x as usize, z as usize) {
                            continue;
                        }
                        // The plan's roof cut: the plane owning this tile at
                        // this draw level (`crate::shadows::interior::roof_hidden`'s rule).
                        let owner = (0..=floor.level).rev().find(|&plane| {
                            scene
                                .tile(plane, x as usize, z as usize)
                                .is_some_and(|t| t.level as usize == floor.level)
                        });
                        if owner.is_some_and(|plane| hidden_plane(plane, x, z)) {
                            continue;
                        }
                        tiles.push(z as usize * tx + x as usize);
                    }
                }
                // The classic roof stamp is the cycle in roof mode 2, so the key
                // moves every cycle: the same tiles keep their indices.
                let kept = self.scene_resources.far.ext[floor.level]
                    .as_mut()
                    .filter(|e| {
                        e.tiles == tiles
                            && e.key.selection == *floor.selection
                            && e.key.far == far_plane
                    });
                if let Some(e) = kept {
                    e.key.roof = roof_key;
                } else {
                    self.scene_resources.far.ext[floor.level] =
                        Some(extension(device, floor, mesh, roof_key, far_plane, tiles));
                }
            }
            if let Some(e) = self.scene_resources.far.ext[floor.level].as_ref() {
                self.scene_resources.far.stats.ext_tiles += e.tiles.len();
                if e.count > 0 {
                    self.scene_resources.far.ext_draws.push(floor.level);
                }
            }
        }
        // The window's containers (a new installed scene starts over).
        let scene_key = (
            crate::terrain::SceneKey::of(snapshot),
            live.static_entity_count,
        );
        if self.scene_resources.far.ext_scene.key.as_ref() != Some(&scene_key) {
            let generation = self.scene_resources.far.ext_scene.generation + 1;
            let mut by_container: BTreeMap<(i32, i32), Vec<usize>> = BTreeMap::new();
            for (id, e) in live
                .entities
                .iter()
                .enumerate()
                .take(live.static_entity_count)
            {
                if e.dynamic || matches!(e.source, EntityRef::Temporary(_)) || e.transparent {
                    continue;
                }
                let (x, z) = (
                    (e.x >> 9) + snapshot.floor_base[0],
                    (e.z >> 9) + snapshot.floor_base[1],
                );
                by_container.entry(container_of(x, z)).or_default().push(id);
            }
            if let Some(jobs) = self.scene_resources.far.loc_jobs.as_mut() {
                jobs.retain_queued(|k| !matches!(k, LocKey::Extension(..)));
            }
            self.scene_resources.far.drop_extension();
            self.scene_resources.far.ext_scene = ExtScene {
                key: Some(scene_key),
                by_container,
                containers: HashMap::new(),
                generation,
            };
        }
        // Locs: static scene-graph entities the plan left out by distance.
        let classic = snapshot.camera.fog_reference().0 - CLASSIC_FAR_MARGIN;
        let square = list
            .floors
            .first()
            .map(|f| (f.selection.origin, f.selection.distance));
        let in_square = |x: i32, z: i32| {
            square.is_some_and(|([ox, oz], d)| {
                x >= ox && z >= oz && x <= ox + 2 * d && z <= oz + 2 * d
            })
        };
        let view = self.scene_resources.far.view;
        let plan = &live.draw.plan;
        let mut planned = std::mem::take(&mut self.scene_resources.far.planned_scratch);
        planned.clear();
        planned.resize(live.entities.len(), false);
        for &id in plan.opaque.iter().chain(&plan.transparent) {
            if let Some(p) = planned.get_mut(id) {
                *p = true;
            }
        }
        let mut selected = std::mem::take(&mut self.scene_resources.far.ext_selected);
        selected.clear();
        selected.resize(live.entities.len(), false);
        #[cfg(test)]
        let per_loc = self.scene_resources.far.test_per_loc;
        #[cfg(not(test))]
        let per_loc = false;
        let mut candidates = std::mem::take(&mut self.scene_resources.far.candidate_scratch);
        candidates.clear();
        let eye = glam::Vec3::from(origin);
        for (id, e) in live.entities.iter().enumerate() {
            if planned[id] || e.dynamic || matches!(e.source, EntityRef::Temporary(_)) {
                continue;
            }
            if snapshot.model(e.source).is_none() {
                continue;
            }
            // The plan's roof cut (`Scene.draw` :1283-1300): the anchor
            // tile, the first footprint tile in the scene.
            if roof.as_ref().is_some_and(|r| {
                e.level >= r.first_plane as i32 && e.occlude_level < scene.max_level as i32
            }) {
                let anchor = (e.tiles[0]..=e.tiles[1])
                    .flat_map(|x| (e.tiles[2]..=e.tiles[3]).map(move |z| (x, z)))
                    .find(|&(x, z)| {
                        x >= 0 && z >= 0 && (x as usize) < scene.max_x && (z as usize) < scene.max_z
                    });
                if anchor.is_some_and(|(x, z)| {
                    roof.as_ref().map(|r| r.stamp) == stamp_at(e.level as usize, x, z)
                }) {
                    continue;
                }
            }
            let (lo, hi) = match e.cylinder {
                Some([x, y, z, min_y, max_y, r]) => (
                    glam::Vec3::new((x - r) as f32, (y + min_y) as f32, (z - r) as f32),
                    glam::Vec3::new((x + r) as f32, (y + max_y) as f32, (z + r) as f32),
                ),
                None => {
                    let p = glam::Vec3::new(e.x as f32, e.y as f32, e.z as f32);
                    (p - glam::Vec3::splat(512.0), p + glam::Vec3::splat(512.0))
                }
            };
            let depth = (view * ((lo + hi) * 0.5 - eye).extend(1.0)).z;
            if in_square(e.x >> 9, e.z >> 9) && depth <= classic {
                continue;
            }
            match box_pixels(
                &self.scene_resources.far.view_proj,
                lo - eye,
                hi - eye,
                self.scene_resources.far.viewport,
            ) {
                Some(px) if px >= MIN_PIXELS => {}
                _ => continue,
            }
            if e.transparent {
                self.scene_resources.far.ext_transparent.push(id);
                continue;
            }
            // Opaque: from its container (below).
            if per_loc {
                self.scene_resources.far.ext_opaque.push(id);
                continue;
            }
            let container = container_of(
                (e.x >> 9) + snapshot.floor_base[0],
                (e.z >> 9) + snapshot.floor_base[1],
            );
            candidates.push((id, container));
        }
        // Request the containers the candidates need that are not built
        // (or went stale: a loc changed), nearest first, within the gather
        // budget (all of them in sync mode, built and uploaded here).
        let mut wanted = std::mem::take(&mut self.scene_resources.far.wanted_scratch);
        wanted.clear();
        for &(_, container) in &candidates {
            let needed = match self
                .scene_resources
                .far
                .ext_scene
                .containers
                .get(&container)
            {
                None | Some(ExtContainer::Gathering { .. }) => true,
                Some(ExtContainer::Ready { stale, .. }) => *stale,
                Some(ExtContainer::Queued { .. }) => false,
            };
            if needed && !wanted.contains(&container) {
                wanted.push(container);
            }
        }
        let focus = [snapshot.camera.target[0], snapshot.camera.target[2]];
        wanted.sort_by_key(|&id| (distance_to_rect(focus, &container_rect(id)), id));
        for (k, &id) in wanted.iter().enumerate() {
            if k > 0 && self.scene_resources.far.over_budget() {
                break;
            }
            self.request_ext_container(snapshot, id, sync);
        }
        self.scene_resources.far.wanted_scratch = wanted;
        if sync {
            self.far_upload(device, queue, true);
        }
        // Each candidate from its container when that holds it as it is
        // now; else per loc (a loc its container left to its classic mesh, or
        // changed since); while its container builds, per loc only if its
        // mesh is already cached (no new build on this thread).
        for &(id, container) in &candidates {
            match self
                .scene_resources
                .far
                .ext_scene
                .containers
                .get_mut(&container)
            {
                Some(ExtContainer::Ready { keys, stale, .. }) => {
                    let current = snapshot.entity_key(id);
                    match keys.get(&(id as u32)) {
                        Some(k) if Some(*k) == current => selected[id] = true,
                        Some(_) => {
                            *stale = true;
                            self.scene_resources.far.ext_opaque.push(id);
                        }
                        None => self.scene_resources.far.ext_opaque.push(id),
                    }
                }
                _ => {
                    if snapshot
                        .entity_key(id)
                        .is_some_and(|k| self.static_model(&k).is_some())
                    {
                        self.scene_resources.far.ext_opaque.push(id);
                    }
                }
            }
        }
        // Draw the containers' selected locs (LOD 0, as the near path).
        let base = snapshot.floor_base;
        let o = glam::Vec3::from(origin);
        let mut run = std::mem::take(&mut self.scene_resources.far.run_scratch);
        let mut records = std::mem::take(&mut self.scene_resources.far.record_scratch);
        records.clear();
        let mut ranges = std::mem::take(&mut self.scene_resources.far.range_scratch);
        ranges.clear();
        let mut ids = std::mem::take(&mut self.scene_resources.far.id_scratch);
        ids.clear();
        ids.extend(
            self.scene_resources
                .far
                .ext_scene
                .containers
                .keys()
                .copied(),
        );
        ids.sort_unstable();
        for id in &ids {
            let Some(ExtContainer::Ready { gpu: Some(gpu), .. }) =
                self.scene_resources.far.ext_scene.containers.get(id)
            else {
                continue;
            };
            for (mesh, alloc) in &gpu.chunks {
                let at = glam::Vec3::new(
                    (mesh.origin[0] - base[0] * 512) as f32,
                    mesh.origin[1] as f32,
                    (mesh.origin[2] - base[1] * 512) as f32,
                ) - o;
                for batch in &mesh.batches {
                    runs(
                        batch,
                        &mesh.models,
                        |m| {
                            selected
                                .get(mesh.models[m as usize].slot as usize)
                                .copied()
                                .unwrap_or(false)
                                .then_some(0)
                        },
                        &mut run,
                    );
                    if !run.is_empty() {
                        records.push(BatchRecord {
                            alloc: *alloc,
                            material: batch.material,
                            at: at.to_array(),
                            lights: batch.lights,
                            transparent: false,
                            depth: 0.0,
                            ranges: (ranges.len(), run.len()),
                        });
                        ranges.extend_from_slice(&run);
                    }
                }
            }
        }
        self.scene_resources.far.run_scratch = run;
        self.scene_resources.far.id_scratch = ids;
        self.scene_resources.far.range_scratch = ranges;
        // By page and material (fewer binds; opaque, so the order changes
        // only exact depth ties).
        records.sort_by_key(|r| (r.alloc.page, r.material, r.alloc.vertex));
        // The extension's opaque batches draw before the far transparent
        // ones.
        let transparent_far = self
            .scene_resources
            .far
            .batch_draws
            .split_off(self.scene_resources.far.batch_opaque);
        for r in records.drain(..) {
            self.scene_resources.far.stats.ext_draws +=
                self.batch_draws(device, queue, snapshot, &r);
        }
        self.scene_resources.far.record_scratch = records;
        self.scene_resources.far.batch_opaque = self.scene_resources.far.batch_draws.len();
        self.scene_resources.far.batch_draws.extend(transparent_far);
        self.scene_resources.far.stats.ext_batched = selected.iter().filter(|&&s| s).count();
        self.scene_resources.far.stats.ext_transparent =
            self.scene_resources.far.ext_transparent.len();
        for c in self.scene_resources.far.ext_scene.containers.values() {
            if let ExtContainer::Ready { gpu: Some(g), .. } = c {
                self.scene_resources.far.stats.ext_containers += 1;
                self.scene_resources.far.stats.ext_bytes += g.bytes;
            }
        }
        self.scene_resources.far.ext_selected = selected;
        self.scene_resources.far.candidate_scratch = candidates;
        self.scene_resources.far.planned_scratch = planned;
        // Far first among the transparent draws.
        let key = |id: &usize| {
            let p = glam::Vec3::new(
                live.entities[*id].x as f32,
                live.entities[*id].y as f32,
                live.entities[*id].z as f32,
            ) - eye;
            -(view * p.extend(1.0)).z
        };
        self.scene_resources
            .far
            .ext_transparent
            .sort_by(|a, b| key(a).total_cmp(&key(b)));
        self.scene_resources.far.stats.ext_locs = self.scene_resources.far.ext_opaque.len()
            + self.scene_resources.far.ext_transparent.len()
            + self.scene_resources.far.stats.ext_batched;
    }

    /// Start extension container `id`'s build: slim copies of its locs'
    /// snapshot models, their RT7 inputs and placements, built on a loc
    /// worker (here, in sync mode).
    fn request_ext_container(&mut self, snapshot: &SceneSnapshot<'_>, id: (i32, i32), sync: bool) {
        let (Some(live), Some(scene), Some(materials)) =
            (snapshot.live_frame(), snapshot.scene, snapshot.materials)
        else {
            return;
        };
        let ids = self
            .scene_resources
            .far
            .ext_scene
            .by_container
            .get(&id)
            .cloned()
            .unwrap_or_default();
        // Resume a gathering, or start one (a stale container's old mesh
        // goes: its locs draw per loc meanwhile).
        let (start, mut locs, mut keys) =
            match self.scene_resources.far.ext_scene.containers.remove(&id) {
                Some(ExtContainer::Gathering { next, locs, keys }) => (next, locs, keys),
                other => {
                    if let Some(ExtContainer::Ready { gpu: Some(old), .. }) = other {
                        old.free(&mut self.scene_resources.far.arena);
                    }
                    (0, Vec::with_capacity(ids.len()), HashMap::new())
                }
            };
        let mut draws = Vec::new();
        for (k, &eid) in ids.iter().enumerate().skip(start) {
            if k > start && self.scene_resources.far.over_budget() {
                self.scene_resources.far.ext_scene.containers.insert(
                    id,
                    ExtContainer::Gathering {
                        next: k,
                        locs,
                        keys,
                    },
                );
                return;
            }
            let e = &live.entities[eid];
            if live.has_dynamic(eid) {
                continue;
            }
            // The near RT7 path's loc and shape (`Rt7Cache::streams`).
            let (loc_id, shape) = match e.source {
                EntityRef::Scenery(i) => {
                    let Some(s) = scene.scenery.get(i).filter(|s| !s.dynamic) else {
                        continue;
                    };
                    (s.loc_id, s.shape)
                }
                EntityRef::Wall(i) => {
                    let Some(s) = scene.walls.get(i).filter(|s| !s.dynamic) else {
                        continue;
                    };
                    (s.loc_id, s.shape)
                }
                EntityRef::WallDecor(i) => {
                    let Some(s) = scene.wall_decors.get(i).filter(|s| !s.dynamic) else {
                        continue;
                    };
                    (
                        s.loc_id,
                        rs910_scene::loctype::shape::WALLDECOR_STRAIGHT_NOOFFSET,
                    )
                }
                EntityRef::GroundDecor(i) => {
                    let Some(s) = scene.ground_decors.get(i).filter(|s| !s.dynamic) else {
                        continue;
                    };
                    (s.loc_id, rs910_scene::loctype::shape::GROUND_DECOR)
                }
                EntityRef::Temporary(_) => continue,
            };
            draws.clear();
            crate::models::draw_list::entity(snapshot, live, scene, eid, &mut draws);
            let Some(d) = draws
                .iter()
                .find(|d| d.kind == Kind::Model && d.key.is_some())
            else {
                continue;
            };
            let Some(classic) = ClassicMesh::of(d.model, materials) else {
                continue;
            };
            keys.insert(eid as u32, d.key.expect("a keyed loc"));
            locs.push(ExtLoc {
                slot: eid as u32,
                loc_id,
                shape,
                classic,
                matrix: d.matrix,
                lights: crate::lighting::point_lights::model_slots(live.model_lights, eid),
            });
        }
        let ext = &mut self.scene_resources.far.ext_scene;
        ext.generation += 1;
        let generation = ext.generation;
        ext.containers
            .insert(id, ExtContainer::Queued { generation, keys });
        let base = snapshot.floor_base;
        let Some(jobs) = self.scene_resources.far.loc_jobs.as_mut() else {
            return;
        };
        if sync {
            if let LocResult::Extension(e) =
                jobs.run_inline(|w| LocResult::Extension(w.extension(id, base, locs)))
            {
                self.scene_resources
                    .far
                    .ready
                    .push_back(Ready::Extension(id, generation, e));
            }
        } else {
            let priority = i64::from(distance_to_rect(
                [snapshot.camera.target[0], snapshot.camera.target[2]],
                &container_rect(id),
            ));
            jobs.submit(LocKey::Extension(id, generation), priority, move |w| {
                LocResult::Extension(w.extension(id, base, locs))
            });
        }
    }

    /// F2/F3: record the near extension's per-loc locs and the merged
    /// meshes' draw packets, opaque (`transparent: None`: the per-loc locs,
    /// then the containers' batches) or transparent (with their ranges and
    /// matrices, far first, ahead of the plan's: the far transparent
    /// batches, then the per-loc locs). None of them casts sun shadows (the
    /// off-screen caster gather already has the window's locs).
    pub(crate) fn prepare_far_locs(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
        transparent: Option<&mut Vec<(std::ops::Range<usize>, [f32; 16])>>,
    ) {
        if !self.scene_resources.far.active {
            return;
        }
        let (Some(live), Some(scene)) = (snapshot.live_frame(), snapshot.scene) else {
            return;
        };
        let ids = if transparent.is_some() {
            std::mem::take(&mut self.scene_resources.far.ext_transparent)
        } else {
            std::mem::take(&mut self.scene_resources.far.ext_opaque)
        };
        let mut ranges = Vec::new();
        if transparent.is_some() {
            // The far transparent batches, far to near.
            for &(d, matrix, bounds) in
                &self.scene_resources.far.batch_draws[self.scene_resources.far.batch_opaque..]
            {
                ranges.push((
                    self.frame_resources.draws.len()..self.frame_resources.draws.len() + 1,
                    matrix,
                ));
                let start = self.frame_resources.draws.len();
                self.frame_resources.draws.push(d);
                self.frame_resources
                    .draw_bounds
                    .push((start as u32, start as u32 + 1, bounds));
            }
        }
        let budget = if self.far_sync() {
            usize::MAX
        } else {
            NEW_MESHES_PER_FRAME
        };
        let mut new = 0;
        let mut draws = Vec::new();
        for &id in &ids {
            draws.clear();
            crate::models::draw_list::entity(snapshot, live, scene, id, &mut draws);
            for entity in &draws {
                // A loc whose mesh is not built yet waits beyond the
                // frame's budget (its RT7 build is the cost).
                if entity.key.is_some_and(|k| self.static_model(&k).is_none()) {
                    if new >= budget {
                        continue;
                    }
                    new += 1;
                }
                let start = self.frame_resources.draws.len();
                let bounds = self.prepare_entity(device, queue, snapshot, entity, origin);
                self.note_bounds(start, bounds);
                for d in &mut self.frame_resources.draws[start..] {
                    d.casts = false;
                }
                ranges.push((
                    start..self.frame_resources.draws.len(),
                    local_matrix(&entity.matrix, origin),
                ));
            }
        }
        if let Some(out) = transparent {
            out.extend(ranges);
            self.scene_resources.far.ext_transparent = ids;
        } else {
            self.scene_resources.far.ext_opaque = ids;
            // The containers' opaque batches.
            let start = self.frame_resources.draws.len();
            self.frame_resources.draws.extend(
                self.scene_resources.far.batch_draws[..self.scene_resources.far.batch_opaque]
                    .iter()
                    .map(|&(d, _, _)| d),
            );
            for (offset, &(_, _, bounds)) in self.scene_resources.far.batch_draws
                [..self.scene_resources.far.batch_opaque]
                .iter()
                .enumerate()
            {
                let index = (start + offset) as u32;
                self.frame_resources
                    .draw_bounds
                    .push((index, index + 1, bounds));
            }
        }
    }

    /// `CLIENT910_MODERN_CHECK`: the extension is disjoint from the plan,
    /// no far tile is inside the window, and every far container was
    /// placed against it.
    pub(crate) fn check_far(&mut self, snapshot: &SceneSnapshot<'_>, list: &DrawList<'_>) {
        if !crate::modern_debug_flags::flags().check {
            return;
        }
        let (ok, report) = self.far_disjoint(snapshot, list);
        if !ok {
            self.scene_resources.far.mismatches += 1;
            log::warn!(
                "[modern] far check: {report} ({} mismatching frames)",
                self.scene_resources.far.mismatches
            );
        } else if self.scene_resources.far.frames == 1
            || self.scene_resources.far.frames.is_multiple_of(600)
        {
            log::info!("[modern] far check: {report}");
        }
    }

    /// Whether the far scene draws nothing the plan draws (see
    /// [`Self::check_far`]), with a report.
    pub(crate) fn far_disjoint(
        &self,
        snapshot: &SceneSnapshot<'_>,
        list: &DrawList<'_>,
    ) -> (bool, String) {
        let Some(live) = snapshot.live_frame() else {
            return (true, "no live scene".into());
        };
        let plan = &live.draw.plan;
        let in_plan = |id: &usize| plan.opaque.contains(id) || plan.transparent.contains(id);
        let batched: Vec<usize> = self
            .scene_resources
            .far
            .ext_selected
            .iter()
            .enumerate()
            .filter(|(_, &s)| s)
            .map(|(id, _)| id)
            .collect();
        let locs = self
            .scene_resources
            .far
            .ext_opaque
            .iter()
            .chain(&self.scene_resources.far.ext_transparent)
            .chain(&batched)
            .filter(|id| in_plan(id))
            .count();
        let mut tiles = 0;
        let mut ext_tiles = 0;
        for floor in &list.floors {
            let Some(Some(e)) = self.scene_resources.far.ext.get(floor.level) else {
                continue;
            };
            let g = floor.geometry;
            ext_tiles += e.tiles.len();
            tiles += e
                .tiles
                .iter()
                .filter(|&&t| {
                    floor
                        .selection
                        .visible((t % g.tiles_x) as i32, (t / g.tiles_x) as i32)
                })
                .count();
        }
        // The far squares' drawn tiles (those outside the window they were
        // uploaded against) against this frame's window.
        let window = snapshot
            .floors
            .first()
            .and_then(Option::as_ref)
            .map_or_else(TileRect::default, |g| {
                TileRect::at(snapshot.floor_base, [g.tiles_x, g.tiles_z])
            });
        let mut inside = 0;
        let mut far_tiles = 0;
        for (id, gpu) in &self.scene_resources.far.squares {
            let Some(mesh) = self
                .scene_resources
                .far
                .world
                .square(*id)
                .and_then(|s| s.value.terrain.as_ref())
            else {
                continue;
            };
            for level in mesh.levels.iter().take(DRAWN_LEVELS).flatten() {
                for (tile, &(_, count)) in level.tile_ranges.iter().enumerate() {
                    let x = id.0 * 64 + (tile % 64) as i32;
                    let z = id.1 * 64 + (tile / 64) as i32;
                    if count > 0 && !gpu.window.contains(x, z) {
                        far_tiles += 1;
                        inside += usize::from(window.contains(x, z));
                    }
                }
            }
        }
        let other_window = self
            .scene_resources
            .far
            .containers
            .values()
            .filter(|c| c.window != window)
            .count();
        let ok = locs == 0 && tiles == 0 && inside == 0 && other_window == 0;
        (
            ok,
            format!(
                "{} extension locs ({} batched; {locs} in the plan), {ext_tiles} extension tiles ({tiles} selected by the plan), {far_tiles} far tiles ({inside} inside the window), {} far loc containers ({other_window} placed against another window)",
                self.scene_resources.far.ext_opaque.len() + self.scene_resources.far.ext_transparent.len() + batched.len(),
                batched.len(),
                self.scene_resources.far.containers.len()
            ),
        )
    }

    /// The far scene's counters of the last frame.
    #[must_use]
    pub fn far_stats(&self) -> FarStats {
        self.scene_resources.far.stats
    }
}

/// Write layer `layer`'s mips `levels` into the far layers' `texture`.
fn write_layer(
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    texture: &wgpu::Texture,
    layer: u32,
    levels: &crate::terrain::atlas::LayerLevels,
) {
    for (mip, (w, h, px)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: mip as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// Start (or restart) square `sq`'s loc build for `view` (built here in
/// sync mode): a new ticket supersedes an outstanding build.
fn far_start_locs(far: &mut FarGpu, sq: SquareId, view: &FarView, sync: bool) {
    far.loc_tickets += 1;
    let ticket = far.loc_tickets;
    let Some(square) = far.world.square_mut(sq) else {
        return;
    };
    square.value.loc_ticket = ticket;
    square.value.loc_level = Some(view.level);
    let window = square.value.window;
    let Some(jobs) = far.loc_jobs.as_mut() else {
        return;
    };
    let (focus, level) = (view.focus, view.level);
    if sync {
        if let LocResult::Square(l) =
            jobs.run_inline(|w| LocResult::Square(w.square(sq, window, focus, level)))
        {
            far.ready.push_back(Ready::Locs(sq, ticket, l));
        }
    } else {
        jobs.retain_queued(|k| !matches!(k, LocKey::Square(s, _) if *s == sq));
        let priority = i64::from(far_ring::distance_to_square(focus, sq));
        jobs.submit(LocKey::Square(sq, ticket), priority, move |w| {
            LocResult::Square(w.square(sq, window, focus, level))
        });
    }
}

/// Square `id`'s buffers: its vertices moved to the scene at `base`, its
/// indices over the tiles outside `window`.
pub(crate) fn upload_square(
    device: &wgpu::Device,
    id: SquareId,
    mesh: &FarMesh,
    base: [i32; 2],
    window: TileRect,
) -> SquareGpu {
    let shift = [
        ((id.0 * 64 - base[0]) * 512) as f32,
        ((id.1 * 64 - base[1]) * 512) as f32,
    ];
    let mut lo = glam::Vec3::splat(f32::MAX);
    let mut hi = glam::Vec3::splat(f32::MIN);
    let mut bytes = 0;
    let levels = mesh
        .levels
        .iter()
        .take(DRAWN_LEVELS)
        .map(|level| {
            let m = level.as_ref()?;
            let mut indices = Vec::new();
            for (tile, &(start, count)) in m.tile_ranges.iter().enumerate() {
                let x = id.0 * 64 + (tile % 64) as i32;
                let z = id.1 * 64 + (tile / 64) as i32;
                if count > 0 && !window.contains(x, z) {
                    indices.extend_from_slice(&m.indices[start as usize..(start + count) as usize]);
                }
            }
            if indices.is_empty() {
                return None;
            }
            let vertices: Vec<TerrainVertex> = m
                .vertices
                .iter()
                .map(|v| {
                    let mut v = *v;
                    v.pos[0] += shift[0];
                    v.pos[2] += shift[1];
                    let p = glam::Vec3::from(v.pos);
                    lo = lo.min(p);
                    hi = hi.max(p);
                    v
                })
                .collect();
            bytes +=
                (vertices.len() * std::mem::size_of::<TerrainVertex>() + indices.len() * 4) as u64;
            let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("modern far terrain vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("modern far terrain indices"),
                contents: bytemuck::cast_slice(&indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            Some((vb, ib, indices.len() as u32))
        })
        .collect();
    SquareGpu {
        levels,
        base,
        window,
        bounds: [lo, hi],
        bytes,
    }
}

impl<'a> EncodeInputs<'a> {
    /// Draw the far scene into `pass` (the forward and normal/depth passes;
    /// the near extension's containers also the water reflection; see the
    /// module docs). Called at the start of the terrain's draws; the caller
    /// re-sets its pipeline and group 1 afterwards.
    pub(crate) fn encode_far<'p>(&self, pass: &mut wgpu::RenderPass<'p>, kind: TerrainPass) {
        let far = self.far;
        let t = self.terrain;
        let (true, Some(pipes)) = (far.active && t.active, t.pipes.as_ref()) else {
            return;
        };
        let terrain_pipeline = match kind {
            TerrainPass::Forward => Some(pipes.forward.current().expect("terrain lit pipeline")),
            TerrainPass::Geometry => Some(&pipes.geometry),
            TerrainPass::Shadow | TerrainPass::Reflection => None,
        };
        if let Some(pipeline) =
            terrain_pipeline.filter(|_| !far.draws.is_empty() || !far.ext_draws.is_empty())
        {
            pass.set_pipeline(pipeline);
            // F2: the window's tiles beyond the plan, from the near terrain.
            if let Some(scene) = t.scene.as_ref() {
                pass.set_bind_group(1, &scene.bind, &[]);
                for &level in &far.ext_draws {
                    let (Some(Some(gpu)), Some(Some(ext))) =
                        (scene.levels.get(level), far.ext.get(level))
                    else {
                        continue;
                    };
                    let Some(indices) = ext.indices.as_ref() else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..ext.count, 0, 0..1);
                }
            }
            // F1: the far squares.
            if let Some(bind) = far.bind.as_ref() {
                pass.set_bind_group(1, bind, &[]);
                for (id, level) in &far.draws {
                    let Some(Some((vertices, indices, count))) =
                        far.squares.get(id).and_then(|s| s.levels.get(*level))
                    else {
                        continue;
                    };
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..*count, 0, 0..1);
                }
            }
        }
    }
}
