//! The shell's active toolkit (selected and recreated on preference
//! changes): the device layer
//! (`rs910_gpu_device::gpu_device::Device`), the faithful GPU toolkit
//! (`rs910_render_gpu::render::Renderer`) and the [`Backend`] that draws
//! the frames: the GPU toolkit itself, the modern renderer's scene inside
//! it (`--renderer modern`), or nothing (`--renderer null`).
//!
//! Programme Phase 4.1: the shell owns the `Device` and lends it to the
//! backend for each call (`&Device` to upload, `&mut Device` to present),
//! so a second GPU backend shares it; the device is created, configured,
//! resized and presented in the same order as when the `Renderer` owned it.
//! The scene-mesh owners reach the faithful toolkit and the device together
//! through [`ActiveToolkit::faithful`]. The backend contract (what a backend
//! receives each frame, in what order, what it answers and may cache) is in
//! `rs910_scene::scene_snapshot`'s module docs.
//!
//! Code-quality programme Phase 3.3/3.4: the choice moved here from the
//! renderer's `software` field. Methods that branch on the backend dispatch
//! here; everything else derefs to the `Renderer` (the device and GPU paths
//! the app drives directly).
//!
//! Two axes (see `docs/renderer/modern-renderer.md`). The toolkit id (`displayMode`) is game state: toolkit 0
//! answers as the software toolkit (no bloom, no
//! anti-aliasing, no post-process capture;
//! `rs910_toolkit::capability::Answers`), and every toolkit draws through
//! the GPU. The software toolkit that drew toolkit 0 was removed (lane
//! DROP-SW): toolkit-0 pixels were never a contract, its answers are.
//! [`RendererKind`] (`--renderer`, never the saved preferences) chooses what
//! draws the *hardware* toolkits' frames: the modern renderer (the default),
//! the faithful GPU toolkit (`--renderer faithful-gpu` or `classic`), or
//! nothing; toolkit 0 is always drawn by the faithful GPU toolkit (or
//! nothing under `--renderer null`), so the settings' display mode still
//! reaches the faithful renderer. Whatever
//! draws, every capability answer comes from the faithful GPU profile
//! ([`RendererKind::capability_profile`],
//! `rs910_toolkit::capability::Profile`), so the UI, CS2 and `ClientOptions`
//! see the same answers (invariants 2 and 4); the session replay's
//! `renderer_choice_is_observationally_inert` gate checks it, for toolkit-0
//! sessions too.
//!
//! The NXT backend (renderer plan M1, `--renderer modern`) is
//! [`RendererKind::Modern`] and a [`Backend::Modern`] holding the
//! `rs910-render-modern` renderer (`ModernRenderer`), created over the shell's
//! `gpu_device::Device` and lent it per call. It draws
//! [`ActiveToolkit::frame_scene`]'s renderer-neutral `SceneSnapshot` with its
//! own resource cache into the scene viewport of a faithful composition
//! (`Renderer::frame_composite`: the faithful UI below and over it, the
//! console, the retained frame, the canvas scale-up, screenshots) and
//! leaves the 2D/UI path (the faithful painter), the loading and
//! message-box frames, the faithful uploads (`faithful`, the skybox's
//! `createModel`) and the capability answers on the faithful GPU toolkit.
//! `CLIENT910_MODERN_CHECK` compares its draw list with the faithful
//! backend's mesh lists each frame (renderer plan M1 verification 2), and
//! its model billboards and particle segments with the ones the faithful
//! backend would draw (M9). The NXT backend receives the faithful toolkit's
//! CPU particle frame ([`ActiveToolkit::prepare_particles`]: the particle
//! owners are shell state the snapshot does not carry) and its bloom state
//! (billboards hide under it).

mod render_thread;

use std::sync::Arc;

use winit::window::Window;

use crate::render::Renderer;
use rs910_toolkit::performance_metric::{Backdrop, Benchmark, RendererProbe};

/// The profiling commands' view of the active renderer, lent to the
/// interface host for a logic cycle (`rs910_toolkit::performance_metric`).
/// `frame` is the client frame the message box aligns to.
pub struct Probe<'a> {
    pub toolkit: &'a mut ActiveToolkit,
    pub frame: [i32; 2],
    /// A screenshot the diagnostics want of the next message box (the
    /// profiling box is drawn and presented inside a logic cycle, where no
    /// frame capture reaches it).
    pub shot: &'a mut dyn FnMut() -> Option<std::path::PathBuf>,
}

impl RendererProbe for Probe<'_> {
    fn canvas_size(&self) -> [u32; 2] {
        let (w, h) = self.toolkit.canvas_size();
        [w, h]
    }
    fn frame_size(&self) -> [i32; 2] {
        self.frame
    }
    fn present_message_box(
        &mut self,
        plan: crate::ui_paint::Plan,
        backdrop: Backdrop,
    ) -> anyhow::Result<()> {
        if let Some(path) = (self.shot)() {
            self.toolkit.request_screenshot(path);
        }
        self.toolkit.frame_message_box_over(plan, backdrop)
    }
    fn benchmark(&mut self, request: &Benchmark<'_>) -> anyhow::Result<i32> {
        self.toolkit.benchmark(request)
    }
}

/// What draws the hardware toolkits' frames (`--renderer`; see the module
/// docs). Toolkit 0 always draws through the faithful GPU toolkit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum RendererKind {
    /// The faithful GPU toolkit (`--renderer faithful-gpu`, or `classic`).
    #[value(alias = "classic")]
    FaithfulGpu,
    /// Nothing is drawn or presented (the UI's frames are digested,
    /// `NullToolkit`); the device still exists for the capability answers.
    Null,
    /// The modern scene renderer (`rs910-render-modern`) inside the faithful
    /// UI; not bound by the pixel invariant. The default.
    #[default]
    #[value(alias = "nxt")]
    Modern,
}

impl RendererKind {
    /// The capability answers of the hardware toolkit this renderer stands
    /// in for, given the faithful GPU toolkit's (`device`, from
    /// `Renderer::capability_profile`). Every renderer answers with the
    /// faithful profile: a different answer would change the saved
    /// `ClientOptions`, the scripts' graphics settings and the anti-aliasing
    /// fallback (invariants 2 and 4). No wildcard arm: a new renderer
    /// (`Nxt`) must decide here.
    pub fn capability_profile(
        self,
        device: rs910_toolkit::capability::Profile,
    ) -> rs910_toolkit::capability::Profile {
        match self {
            RendererKind::FaithfulGpu => device,
            RendererKind::Null => device,
            // It draws the scene only; the toolkit the game sees is the
            // faithful GPU toolkit (renderer plan §5 "Inertness").
            RendererKind::Modern => device,
        }
    }
}

/// Which toolkit draws the frames.
pub enum Backend {
    /// The faithful GPU toolkit draws (the hardware toolkits 1/5 by default,
    /// and toolkit 0 under every renderer but `--renderer null`).
    Gpu,
    /// `--renderer null`: nothing is drawn or presented.
    Null(NullFrames),
    /// `--renderer modern`: the NXT renderer draws the scene, the faithful
    /// toolkit everything else.
    Modern(Box<rs910_render_modern::frame::ModernRenderer>),
    /// The sole modern renderer and surface are owned by the render thread.
    Pending,
}

/// `--renderer null`'s record of what it was asked to draw: the frames and
/// the last UI frame's op digest (`rs910_toolkit::toolkit::digest`, the
/// null toolkit's digest of its 2D calls).
#[derive(Default)]
pub struct NullFrames {
    pub frames: u64,
    pub ui_digest: u64,
    /// A `--screenshot` was requested: there are no pixels, so it counts as
    /// written (the app's screenshot exit still fires).
    screenshot: bool,
}

/// The renderer the app drives: see the module docs.
pub struct ActiveToolkit {
    gpu: Renderer,
    /// The device layer the backends share (Phase 4.1): the window surface,
    /// device and queue, lent to the backend that draws each call.
    device: crate::gpu_device::Device,
    backend: Backend,
    /// `--renderer`.
    kind: RendererKind,
    /// The active toolkit's capability answers: whether toolkit 0
    /// (`displayMode` 0) is active, and the hardware toolkit's profile
    /// ([`RendererKind::capability_profile`]).
    answers: rs910_toolkit::capability::Answers,
    /// The faithful toolkit's bloom state (`Renderer::set_scene_effects`'
    /// `bloom` once it succeeded): the NXT backend's billboard set follows
    /// it (renderer plan M9).
    scene_bloom: bool,
    /// The NXT backend's forward-target sample count (the anti-aliasing
    /// level it follows, [`ActiveToolkit::modern_follow_samples`]); `None`
    /// until a level arrives (the device's default then).
    modern_samples: Option<u32>,
    /// The modern backend being created on a background thread (started
    /// when the first anti-aliasing level arrives, taken at its first use:
    /// `rs910_render_modern::frame::startup`).
    modern_startup: Option<rs910_render_modern::frame::startup::Startup>,
    /// The window scale and the saved/effective local quality records.
    /// Startup, recreation and live edits share this preference owner.
    display_scale_factor: f64,
    modern_preferences: rs910_config::renderer_preferences::RendererPreferences,
    modern_settings: rs910_render_modern::settings::ModernSettings,
    /// The window the device draws to, kept to make the surface again after
    /// a device loss (none for a headless toolkit).
    window: Option<Arc<Window>>,
    /// How many devices this toolkit has had after the first.
    generation: u32,
    render_thread: Option<render_thread::RenderThread>,
    snapshots: rs910_scene::scene_snapshot::owned::SnapshotBuilder,
    snapshot_spare: Option<rs910_scene::scene_snapshot::owned::OwnedSnapshot>,
    next_particles: Option<rs910_render_modern::sprites::particles::ParticleFrame>,
    next_shadows: Option<rs910_render_modern::shadows::Settings>,
    frame_id: u64,
    cycle_id: i32,
}

/// How [`ActiveToolkit::recover`] answers a device fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recovery {
    /// A new device and new resources for the same renderer.
    Recreate,
    /// A new device, and the faithful GPU renderer in place of the modern
    /// one: the safer of the two.
    SaferRenderer,
}

impl std::ops::Deref for ActiveToolkit {
    type Target = Renderer;
    fn deref(&self) -> &Renderer {
        &self.gpu
    }
}

impl std::ops::DerefMut for ActiveToolkit {
    fn deref_mut(&mut self) -> &mut Renderer {
        &mut self.gpu
    }
}

/// `CLIENT910_MODERN_CHECK` (renderer plan M1 verification 2): the NXT draw
/// list of `snapshot` against the faithful backend's mesh lists for the
/// same frame (`SceneSnapshot` in, `SceneMeshes::frame_summary` out: the
/// meshes the faithful frame would draw). Entity draws are compared as
/// `(plan id, slot)` with the faithful slots (0 spot shadow, 1-9 hint
/// arrows, 10 body or model; hint arrows as one class), floors as `(level,
/// material, index count)` per drawn batch. Logs the counts and the first
/// difference.
fn check_modern_draw_list(
    snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
    meshes: &crate::scene_meshes::SceneMeshes,
    players: Option<&crate::player_renderer::PlayersRenderer>,
) {
    use rs910_render_modern::models::draw_list::{DrawList, Kind};
    static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static MISMATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let modern = DrawList::build(snapshot).summary();
    let faithful = meshes.frame_summary(snapshot, players);
    let class = |slot: usize| match slot {
        0 | 10 => slot,
        _ => 1,
    };
    let modern_entities = |list: &[(usize, Kind)]| -> Vec<(usize, usize)> {
        list.iter()
            .map(|&(id, kind)| {
                let slot = match kind {
                    Kind::SpotShadow => 0,
                    Kind::HintArrow => 1,
                    Kind::Body | Kind::Model => 10,
                };
                (id, slot)
            })
            .collect()
    };
    let faithful_entities = |list: &[(usize, usize)]| -> Vec<(usize, usize)> {
        list.iter().map(|&(id, slot)| (id, class(slot))).collect()
    };
    let pairs = [
        (
            "opaque",
            modern_entities(&modern.opaque),
            faithful_entities(&faithful.opaque),
        ),
        (
            "transparent",
            modern_entities(&modern.transparent),
            faithful_entities(&faithful.transparent),
        ),
    ];
    let floors_modern: Vec<(usize, i32, u32)> = modern
        .floors
        .iter()
        .map(|&(level, material, count)| (level, material, count as u32))
        .collect();
    let frame = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let mut same = floors_modern == faithful.floors;
    for (name, a, b) in &pairs {
        if a != b {
            same = false;
            let first = a.iter().zip(b).position(|(x, y)| x != y);
            log::warn!(
                "[modern-check] frame {frame}: {name} lists differ: modern {} vs faithful {} draws, first difference at {first:?}",
                a.len(),
                b.len()
            );
        }
    }
    if floors_modern != faithful.floors {
        log::warn!(
            "[modern-check] frame {frame}: floors differ: modern {} vs faithful {} batches",
            floors_modern.len(),
            faithful.floors.len()
        );
    }
    if !same {
        MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if frame == 1 || frame.is_multiple_of(100) || !same {
        log::info!(
            "[modern-check] frame {frame}: {} opaque + {} transparent draws, {} floor batches ({} indices); {} of {frame} frames differed from the faithful lists",
            pairs[0].1.len(),
            pairs[1].1.len(),
            floors_modern.len(),
            floors_modern.iter().map(|f| u64::from(f.2)).sum::<u64>(),
            MISMATCHES.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

/// The faithful toolkit's CPU particle frame as the NXT backend's input
/// (renderer plan M9, `rs910_render_modern::sprites::particles`): the same quads,
/// batches and owner segments.
fn modern_particle_frame(
    frame: &crate::particle_render::Frame,
) -> rs910_render_modern::sprites::particles::ParticleFrame {
    use rs910_render_modern::sprites::particles::{
        ParticleBatch, ParticleFrame, ParticleSegment, ParticleVertex,
    };
    ParticleFrame {
        vertices: frame
            .vertices
            .iter()
            .map(|v| ParticleVertex {
                pos: v.pos,
                colour: v.colour,
                uv: v.uv,
            })
            .collect(),
        batches: frame
            .batches
            .iter()
            .map(|b| ParticleBatch {
                texture: b.texture,
                lit: b.lit,
                first_vertex: b.first_vertex,
                quads: b.quads,
            })
            .collect(),
        segments: frame
            .segments
            .iter()
            .map(|s| ParticleSegment {
                list: s.list,
                entity: s.entity,
                batches: s.batches.clone(),
            })
            .collect(),
    }
}

/// The faithful mesh of plan entity `id`'s slot `slot` (`SceneMeshes::
/// mesh_for`'s slots: 0 the spot shadow, 1-9 the hint arrows, 10 the body
/// or model), for the `CLIENT910_MODERN_CHECK` billboard comparison.
fn faithful_mesh<'m>(
    snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
    meshes: &'m crate::scene_meshes::SceneMeshes,
    players: Option<&'m crate::player_renderer::PlayersRenderer>,
    id: usize,
    slot: usize,
) -> Option<&'m crate::floor_render::FloorMesh> {
    let pick = |ids: &[Option<(bool, usize)>],
                opaque: &'m [crate::floor_render::FloorMesh],
                transparent: &'m [crate::floor_render::FloorMesh]| {
        let (t, i) = ids.get(id).copied().flatten()?;
        if t {
            transparent.get(i)
        } else {
            opaque.get(i)
        }
    };
    let e = snapshot.live?.entities.get(id)?;
    if let crate::scene::EntityRef::Temporary(index) = e.source {
        if snapshot
            .scene
            .is_some_and(|scene| scene.temporary[index].transient)
        {
            return (slot == 10).then(|| {
                pick(
                    &meshes.transient_mesh_ids,
                    &meshes.transient_opaque_meshes,
                    &meshes.transient_transparent_meshes,
                )
            })?;
        }
        let entry = players?.meshes.get(&(e.loc_id as usize))?;
        return match slot {
            0 => entry.shadow.as_ref().and_then(|s| s.mesh.as_ref()),
            1..=9 => entry
                .hint_arrows
                .get(slot - 1)
                .and_then(|a| a.mesh.as_ref()),
            _ => entry.mesh.as_ref(),
        };
    }
    (slot == 10).then(|| {
        pick(
            &meshes.scene_mesh_ids,
            &meshes.scene_opaque_meshes,
            &meshes.scene_transparent_meshes,
        )
    })?
}

/// `CLIENT910_MODERN_CHECK` for the model billboards and particles (renderer
/// plan M9), after the NXT frame: the billboards the faithful backend would
/// draw for this snapshot (`billboard_render::Frame::add` over its meshes in
/// its list order, `Renderer::prepare_billboards`' camera and bloom) against
/// the NXT frame's (`nxt_billboards`): the same segments (list, owner
/// position, quad count) and per quad the same material, alpha reference,
/// depth writes, colour and corners (within 0.5 fine units); and each
/// particle owner's segment placed after the same owner draws
/// (`particle_render::Segment::resolve` over the faithful mesh counts)
/// with every quad of its batches drawn. Logs the counts and the first
/// difference.
fn check_modern_sprites(
    snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
    meshes: &crate::scene_meshes::SceneMeshes,
    players: Option<&crate::player_renderer::PlayersRenderer>,
    modern: &rs910_render_modern::frame::ModernRenderer,
    bloom: bool,
) {
    static FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    static MISMATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let frame = FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    let summary = meshes.frame_summary(snapshot, players);
    // The faithful billboards.
    let mut local = snapshot.camera.clone();
    local.target = [0; 3];
    let view = rs910_render_gpu::billboard_render::View::new(
        local.view_entries(),
        &local.projection(),
        snapshot.env.fog.range,
        bloom,
    );
    let target = snapshot.camera.target.map(f64::from);
    let mut faithful = rs910_render_gpu::billboard_render::Frame::default();
    for (list, entries) in [&summary.opaque, &summary.transparent]
        .into_iter()
        .enumerate()
    {
        for (index, &(id, slot)) in entries.iter().enumerate() {
            if let Some(b) = faithful_mesh(snapshot, meshes, players, id, slot)
                .and_then(|m| m.billboards.as_ref())
            {
                faithful.add(list as u8, index + 1, b, &b.world(target), &view);
            }
        }
    }
    let ours = modern.billboards();
    let mut problems = Vec::new();
    let segments = |list: u8, at: usize, n: usize| (list, at, n);
    let a: Vec<_> = ours
        .segments
        .iter()
        .map(|s| segments(s.list, s.at, s.quads.len()))
        .collect();
    let b: Vec<_> = faithful
        .segments
        .iter()
        .map(|s| segments(s.list, s.at, s.draws.len()))
        .collect();
    if a != b {
        let first = a.iter().zip(&b).position(|(x, y)| x != y);
        problems.push(format!(
            "billboard segments differ: modern {} vs faithful {}, first difference at {first:?}",
            a.len(),
            b.len()
        ));
    } else {
        for (i, (q, d)) in ours.quads.iter().zip(&faithful.draws).enumerate() {
            let v = &faithful.vertices[d.first_vertex as usize..d.first_vertex as usize + 4];
            let corner_error = q
                .corners
                .iter()
                .zip(v)
                .flat_map(|(c, v)| (0..3).map(move |k| (c[k] - v.pos[k]).abs()))
                .fold(0.0_f32, f32::max);
            if q.material != d.material
                || q.alpha_ref != d.alpha_ref
                || q.depth_write != d.depth_write
                || q.colour != v[0].colour
                || corner_error > 0.5
            {
                problems.push(format!(
                    "billboard {i} differs: modern m{} ref {} depth {} rgba {:?} vs faithful m{} ref {} depth {} rgba {:?}; corners off by {corner_error}",
                    q.material, q.alpha_ref, q.depth_write, q.colour,
                    d.material, d.alpha_ref, d.depth_write, v[0].colour
                ));
                break;
            }
        }
    }
    // The particles: each owner after the same faithful meshes.
    let particles = modern.particles();
    let expected: Vec<(u8, usize, usize)> = if particles.batches.is_empty() {
        Vec::new()
    } else if particles.segments.is_empty() {
        vec![(1, summary.transparent.len(), particles.quads())]
    } else {
        let ends = |ids: &[usize], entries: &[(usize, usize)]| -> Vec<usize> {
            let mut at = 0;
            ids.iter()
                .map(|&id| {
                    while entries.get(at).is_some_and(|&(e, _)| e == id) {
                        at += 1;
                    }
                    at
                })
                .collect()
        };
        let (opaque, transparent) = snapshot.live.map_or((Vec::new(), Vec::new()), |live| {
            (
                ends(&live.draw.plan.opaque, &summary.opaque),
                ends(&live.draw.plan.transparent, &summary.transparent),
            )
        });
        particles
            .segments
            .iter()
            .map(|s| {
                let (ends, len) = if s.list == 0 {
                    (&opaque, summary.opaque.len())
                } else {
                    (&transparent, summary.transparent.len())
                };
                let at = ends.get(s.entity).copied().unwrap_or(usize::MAX).min(len);
                let quads = particles.batches[s.batches.clone()]
                    .iter()
                    .map(|b| b.quads as usize)
                    .sum();
                (s.list.min(1), at, quads)
            })
            .collect()
    };
    let drawn: Vec<(u8, usize, usize)> = modern
        .sprite_segments()
        .iter()
        .filter(|s| s.particles)
        .map(|s| (s.list, s.at, s.quads))
        .collect();
    if drawn != expected {
        let first = drawn.iter().zip(&expected).position(|(x, y)| x != y);
        problems.push(format!(
            "particle segments differ: modern {} vs faithful {}, first difference at {first:?}",
            drawn.len(),
            expected.len()
        ));
    }
    for problem in &problems {
        log::warn!("[modern-check] frame {frame}: {problem}");
    }
    if !problems.is_empty() {
        MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    if frame == 1 || frame.is_multiple_of(100) || !problems.is_empty() {
        log::info!(
            "[modern-check] frame {frame}: {} billboards on {} models, {} particles of {} owners; {} of {frame} frames differed from the faithful billboards/particles",
            ours.quads.len(),
            ours.segments.len(),
            drawn.iter().map(|d| d.2).sum::<usize>(),
            drawn.len(),
            MISMATCHES.load(std::sync::atomic::Ordering::Relaxed)
        );
    }
}

impl ActiveToolkit {
    /// Immutable diagnostic identity of the next captured scene.
    pub fn set_frame_identity(&mut self, frame: u64, cycle: i32) {
        self.frame_id = frame;
        self.cycle_id = cycle;
    }

    fn enqueue(
        &mut self,
        modern: Box<rs910_render_modern::frame::ModernRenderer>,
        composition: rs910_render_gpu::render::Composition,
        snapshot: Option<rs910_scene::scene_snapshot::owned::OwnedSnapshot>,
    ) {
        let context = self.device.upload_context();
        let device = std::mem::replace(&mut self.device, context);
        self.render_thread
            .get_or_insert_with(render_thread::RenderThread::new)
            .submit(render_thread::Job {
                device,
                modern,
                composition,
                snapshot,
                frame: self.frame_id,
                cycle: self.cycle_id,
                queued: std::time::Instant::now(),
                profile: rs910_core::profile::thread_context(),
            });
    }
    fn accept_render(&mut self, completed: render_thread::Completed) -> anyhow::Result<()> {
        let render_thread::Completed {
            job,
            result,
            profile,
        } = completed;
        rs910_core::profile::merge_thread(profile);
        self.device = job.device;
        self.backend = Backend::Modern(job.modern);
        self.gpu.restore_composition(job.composition);
        if let Some(snapshot) = job.snapshot {
            self.snapshot_spare = Some(snapshot);
        }
        result
    }
    fn finish_render(&mut self) -> anyhow::Result<()> {
        if matches!(self.backend, Backend::Pending) {
            let completed = rs910_core::profile::scope!(
                "render backpressure",
                self.render_thread
                    .as_ref()
                    .expect("in-flight frame")
                    .receive()
            );
            self.accept_render(completed)?;
        }
        Ok(())
    }
    fn render_barrier(&mut self) {
        if let Err(error) = self.finish_render() {
            crate::logging::warn_repeated!("[client910] frame failed: {error:#}");
        }
    }
    /// Observe ordered completions before the next logic/screenshot exit check.
    pub fn poll_render(&mut self) {
        if matches!(self.backend, Backend::Pending) {
            if let Some(completed) = self.render_thread.as_ref().expect("in-flight frame").poll() {
                if let Err(error) = self.accept_render(completed) {
                    crate::logging::warn_repeated!("[client910] frame failed: {error:#}");
                }
            }
        }
    }

    /// Waits until the GPU has finished all submitted work (shutdown: the
    /// device, surface and backends then drop with nothing in flight;
    /// `shutdown_signal` module docs).
    pub fn wait_idle(&mut self) {
        self.render_barrier();
        self.device.wait_idle();
    }

    /// Create the device layer for `window` and the GPU toolkit over it
    /// (the former `Renderer::new(window)`); the hardware backend of `kind`
    /// draws until the session installs its toolkit.
    pub async fn new(window: Arc<Window>, kind: RendererKind) -> anyhow::Result<Self> {
        let inner = window.inner_size();
        let size = (inner.width, inner.height);
        let target = Arc::clone(&window);
        let device = Self::create_device(Some(target), size, kind).await?;
        let mut toolkit = Self::over(device, kind);
        toolkit.window = Some(window);
        Ok(toolkit)
    }

    /// A toolkit on a device with no window: its frames are skipped. For
    /// tests of the toolkit's resources and of device recovery.
    #[cfg(test)]
    pub async fn headless(size: (u32, u32), kind: RendererKind) -> anyhow::Result<Self> {
        let device = Self::create_device(None, size, kind).await?;
        Ok(Self::over(device, kind))
    }

    /// The device layer for `kind`'s renderer, over `target` (none:
    /// headless).
    async fn create_device(
        target: Option<Arc<Window>>,
        size: (u32, u32),
        kind: RendererKind,
    ) -> anyhow::Result<crate::gpu_device::Device> {
        // The NXT renderer's material textures use BC or ETC2 blocks when
        // the adapter has them (renderer plan M2); the faithful renderers'
        // device is unchanged.
        let optional = if kind == RendererKind::Modern {
            rs910_render_modern::device_features::optional_device_features()
        } else {
            Default::default()
        };
        match target {
            Some(target) => {
                crate::gpu_device::Device::new_with_features(
                    target,
                    size,
                    Renderer::device_options(),
                    optional,
                )
                .await
            }
            None => {
                crate::gpu_device::Device::headless(size, Renderer::device_options(), optional)
                    .await
            }
        }
    }

    /// The toolkit over `device`: the GPU renderer and the hardware backend of
    /// `kind` until the session installs its toolkit.
    fn over(device: crate::gpu_device::Device, kind: RendererKind) -> Self {
        let gpu = Renderer::new(&device);
        let profile = kind.capability_profile(gpu.capability_profile(&device));
        let toolkit = Self {
            gpu,
            device,
            backend: Self::hardware_backend(kind),
            kind,
            answers: rs910_toolkit::capability::Answers::hardware(profile),
            scene_bloom: false,
            modern_samples: None,
            display_scale_factor: 1.0,
            modern_preferences: Default::default(),
            modern_settings: rs910_render_modern::settings::ModernSettings::from_env(),
            modern_startup: None,
            window: None,
            generation: 0,
            render_thread: None,
            snapshots: Default::default(),
            snapshot_spare: None,
            next_particles: None,
            next_shadows: None,
            frame_id: 0,
            cycle_id: 0,
        };
        // The NXT backend is created once, with the session's
        // anti-aliasing level: on a background thread from when the level
        // arrives (`modern_follow_samples`), taken at its first use
        // (`ensure_modern`).
        toolkit
    }

    /// The next thing wrong with the device that the client must answer
    /// (a loss, an allocation failure), each handed out once.
    pub fn take_fault(&self) -> Option<rs910_gpu_device::health::Fault> {
        self.device.take_fault()
    }

    /// The renderer that draws the hardware toolkits' frames.
    pub fn kind(&self) -> RendererKind {
        self.kind
    }

    /// Answer a device fault: make a new device and surface, and a new
    /// toolkit over them, as a toolkit change does. Everything created on the
    /// old device is dropped; the owners of uploads (the scene's meshes, the
    /// players, the sky) must be reset by the caller, which rebuilds the
    /// scene. [`Recovery::SaferRenderer`] also replaces the modern renderer
    /// by the faithful GPU renderer. The window's scale and the saved render
    /// scale carry over; the toolkit 0 answers follow what was active.
    ///
    /// On an error nothing changed except that the modern backend, if it
    /// was drawing, is gone.
    pub fn recover(&mut self, recovery: Recovery) -> anyhow::Result<()> {
        self.render_barrier();
        // A modern backend still being made on its start-up thread is not
        // wanted: it was made for the old device.
        self.gpu.set_threaded_composition(false);
        drop(self.modern_startup.take());
        self.backend = Backend::Gpu;
        if recovery == Recovery::SaferRenderer && self.kind == RendererKind::Modern {
            self.kind = RendererKind::FaithfulGpu;
        }
        let target = self.window.clone();
        pollster::block_on(self.device.recreate(target))?;
        self.gpu = Renderer::new(&self.device);
        self.snapshots = Default::default();
        self.snapshot_spare = None;
        self.next_particles = None;
        self.next_shadows = None;
        self.gpu.set_ui_scale(self.display_scale_factor);
        let toolkit0 = self.answers.toolkit0;
        self.answers = rs910_toolkit::capability::Answers::hardware(
            self.kind
                .capability_profile(self.gpu.capability_profile(&self.device)),
        );
        self.answers.toolkit0 = toolkit0;
        self.scene_bloom = false;
        self.backend = Self::hardware_backend(self.kind);
        self.generation += 1;
        log::info!(
            "[client910] GPU device {} on {}",
            self.generation + 1,
            self.device.adapter_info()
        );
        Ok(())
    }

    /// A toolkit change on the native device: the old device is disposed and
    /// a new one made with everything on it, as the original client does,
    /// and toolkit 0's answers follow `toolkit0`. The caller resets the
    /// owners of uploads ([`ActiveToolkit::recover`]) and applies the
    /// anti-aliasing and bloom levels ([`ActiveToolkit::recreate_toolkit_targets`]).
    pub fn recreate_device(&mut self, toolkit0: bool) -> anyhow::Result<()> {
        self.recover(Recovery::Recreate)?;
        self.answers.toolkit0 = toolkit0;
        Ok(())
    }

    /// How many devices this toolkit has had after the first: each toolkit
    /// change and each answered fault makes a new one.
    pub fn device_generation(&self) -> u32 {
        self.generation
    }

    /// Lose the device, as a driver reset does: the device is destroyed and
    /// reports itself lost (`CLIENT910_LOSE_DEVICE`, and the tests of the
    /// recovery).
    pub fn lose_device(&self) {
        self.device.device.destroy();
    }

    /// A new modern backend over the device: its forward target takes the
    /// anti-aliasing level's sample count it follows, else 4x MSAA when the
    /// device supports it for the HDR format (the faithful HDR sample set,
    /// `Device::scene_sample_counts`); its quality settings are the defaults
    /// with the `CLIENT910_MODERN_*` development overrides
    /// (`ModernSettings::from_env`, read when the backend is created, once).
    fn new_modern(&mut self) -> Backend {
        let samples = self.modern_forward_samples();
        if let Some(startup) = self.modern_startup.take() {
            let start = std::time::Instant::now();
            let renderer = startup.finish(&self.device.device, &self.device.queue, samples);
            log::info!(
                "[client910] modern renderer: {samples}x MSAA forward target (taken from its startup thread in {:.1} ms)",
                start.elapsed().as_secs_f64() * 1000.0
            );
            return self.modern_backend(renderer);
        }
        let start = std::time::Instant::now();
        let renderer = rs910_render_modern::frame::ModernRenderer::new(
            &self.device.device,
            &self.device.queue,
            self.device.config.format,
            samples,
            self.modern_settings,
        );
        log::info!(
            "[client910] modern renderer: {samples}x MSAA forward target (created in {:.1} ms)",
            start.elapsed().as_secs_f64() * 1000.0
        );
        self.modern_backend(renderer)
    }

    /// The forward target's sample count the modern renderer is made with:
    /// the level the shell chose, else 4x when the device has it.
    fn modern_forward_samples(&self) -> u32 {
        self.modern_samples.unwrap_or_else(|| {
            let (_, hdr) = self
                .device
                .scene_sample_counts(rs910_render_modern::frame::DEPTH_FORMAT);
            if hdr.contains(&4) {
                4
            } else {
                1
            }
        })
    }

    /// The modern backend of `renderer`, told the window's scale factor and the
    /// saved render scale.
    fn modern_backend(&self, mut renderer: rs910_render_modern::frame::ModernRenderer) -> Backend {
        renderer.set_quality(self.modern_settings);
        renderer.set_display(self.display_scale_factor, None);
        Backend::Modern(Box::new(renderer))
    }

    /// `Renderer::set_ui_scale` (the window's scale factor); the modern
    /// backend takes it for its automatic render scale.
    pub fn set_ui_scale(&mut self, scale: f64) {
        self.render_barrier();
        self.gpu.set_ui_scale(scale);
        self.display_scale_factor = scale;
        self.apply_modern_display();
    }

    /// Keep automatic scene resolution in step with the display.
    fn apply_modern_display(&mut self) {
        if let Backend::Modern(modern) = &mut self.backend {
            modern.set_display(self.display_scale_factor, None);
        }
    }

    pub fn modern_preferences(&self) -> rs910_config::renderer_preferences::RendererPreferences {
        self.modern_preferences
    }

    pub fn effective_modern_preferences(
        &self,
    ) -> rs910_config::renderer_preferences::RendererPreferences {
        self.modern_settings.preferences()
    }

    pub fn modern_controls_available(&self) -> bool {
        self.kind == RendererKind::Modern && !self.answers.toolkit0
    }

    /// Finish an in-flight frame only when its effective quality changes.
    /// Recreation, metric probes and startup use this same resolved state.
    pub fn set_modern_preferences(
        &mut self,
        preferences: rs910_config::renderer_preferences::RendererPreferences,
    ) {
        let settings = rs910_render_modern::settings::ModernSettings::resolve(preferences);
        self.modern_preferences = preferences;
        if settings == self.modern_settings {
            return;
        }
        self.render_barrier();
        self.modern_settings = settings;
        if let Backend::Modern(modern) = &mut self.backend {
            modern.set_quality(settings);
        }
    }

    /// Create the NXT backend if `--renderer modern` draws the hardware
    /// toolkit and it does not exist yet (its first use).
    fn ensure_modern(&mut self) {
        if self.kind == RendererKind::Modern
            && !self.answers.toolkit0
            && matches!(self.backend, Backend::Gpu)
        {
            self.backend = self.new_modern();
        }
    }

    /// What draws a hardware toolkit's frames under `kind`.
    fn hardware_backend(kind: RendererKind) -> Backend {
        match kind {
            RendererKind::FaithfulGpu => Backend::Gpu,
            RendererKind::Null => Backend::Null(NullFrames::default()),
            // Created by `ensure_modern`/`set_toolkit0` over the device.
            RendererKind::Modern => Backend::Gpu,
        }
    }

    /// Toolkit selection: whether toolkit 0
    /// (`displayMode` 0, the software toolkit) or a hardware toolkit
    /// (1/5) is active. It sets the toolkit-0 answers
    /// ([`rs910_toolkit::capability::Answers`]); the faithful GPU toolkit
    /// draws toolkit 0, so only `--renderer modern` changes backend: its
    /// scene renderer draws the hardware toolkits only. `--renderer null`
    /// keeps drawing nothing.
    pub fn set_toolkit0(&mut self, toolkit0: bool) {
        if toolkit0 == self.answers.toolkit0 {
            return;
        }
        self.render_barrier();
        self.gpu.set_threaded_composition(false);
        self.answers.toolkit0 = toolkit0;
        if self.kind == RendererKind::Modern {
            self.gpu.invalidate_retained_frame();
        }
        match (self.kind, toolkit0) {
            (RendererKind::FaithfulGpu | RendererKind::Null, _) => {}
            (RendererKind::Modern, true) => self.backend = Backend::Gpu,
            (RendererKind::Modern, false) => self.backend = self.new_modern(),
        }
    }
    /// Toolkit 0 (the software toolkit) is the active toolkit, whatever `--renderer` draws with.
    pub fn toolkit0(&self) -> bool {
        self.answers.toolkit0
    }

    pub fn prepare_ui(
        &mut self,
        output: crate::ui_output::Output<crate::interface_model::Draw>,
    ) -> anyhow::Result<()> {
        match &mut self.backend {
            Backend::Null(null) => {
                null.ui_digest = rs910_toolkit::toolkit::digest(&output.recording.ops);
                return Ok(());
            }
            Backend::Gpu | Backend::Modern(_) | Backend::Pending => {}
        }
        self.ensure_modern();
        self.gpu.set_threaded_composition(
            self.kind == RendererKind::Modern
                && !self.toolkit0()
                && !rs910_render_modern::modern_debug_flags::flags().check,
        );
        self.gpu.prepare_ui(&self.device, output)
    }

    /// `MessageBox.draw(text, true, toolkit, ..)` in a rebuild state
    /// (`Renderer::frame_message_box`), over the retained last frame.
    pub fn frame_message_box(&mut self, plan: crate::ui_paint::Plan) -> anyhow::Result<()> {
        self.frame_message_box_over(plan, Backdrop::LastFrame)
    }

    /// [`Self::frame_message_box`] over `backdrop`.
    pub fn frame_message_box_over(
        &mut self,
        plan: crate::ui_paint::Plan,
        backdrop: Backdrop,
    ) -> anyhow::Result<()> {
        self.render_barrier();
        if let Backend::Null(null) = &mut self.backend {
            null.frames += 1;
            return Ok(());
        }
        self.gpu
            .frame_message_box_over(&mut self.device, plan, backdrop)
    }

    /// The benchmark of the performance metric on this renderer's device:
    /// what `--renderer` draws the hardware toolkits with also draws the
    /// benchmark (the modern renderer through its own passes, the faithful
    /// toolkit and `--renderer null` through the faithful benchmark).
    pub fn benchmark(&mut self, request: &Benchmark<'_>) -> anyhow::Result<i32> {
        self.render_barrier();
        match self.kind {
            RendererKind::Modern => rs910_render_modern::frame::benchmark::measure(
                &self.device.device,
                &self.device,
                self.device.config.format,
                self.modern_forward_samples(),
                self.modern_settings,
                request,
            ),
            RendererKind::FaithfulGpu | RendererKind::Null => {
                crate::ui_preferences_metric_gpu::measure(
                    &self.device.device,
                    &self.device.queue,
                    request,
                )
            }
        }
    }

    /// The minimap base rebuild (`Renderer::render_minimap_base`).
    pub fn render_minimap_base(
        &mut self,
        plan: &crate::minimap::BasePlan,
        geometries: &[Option<crate::floor::FloorGeometry>],
        meshes: &mut [Option<crate::floor_render::FloorMesh>],
        base: (i32, i32),
        size_z: i32,
        env: &crate::env::EnvFrame,
    ) -> anyhow::Result<u64> {
        self.render_barrier();
        if let Backend::Null(_) = self.backend {
            // Nothing draws: the base sprite is only an id.
            let id = *self.gpu.next_external_id();
            *self.gpu.next_external_id() += 1;
            return Ok(id);
        }
        self.gpu.render_minimap_base(
            &mut self.device,
            plan,
            rs910_render_gpu::render::MinimapWorld {
                geometries,
                meshes,
                base,
                size_z,
            },
            env,
        )
    }

    /// Drop a base sprite.
    pub fn release_minimap_base(&mut self, id: u64) {
        self.render_barrier();
        self.gpu.release_minimap_base(id);
    }

    pub fn prepare_console(
        &mut self,
        console: &dyn crate::console_draw::ConsoleView,
        cycle: i32,
        focused: bool,
    ) -> anyhow::Result<()> {
        if let Backend::Null(_) = self.backend {
            return Ok(());
        }
        self.gpu
            .prepare_console(&self.device, console, cycle, focused)
    }

    /// The scene draw and the frame: the active backend draws this
    /// frame's renderer-neutral scene (`snapshot`, renderer plan A1) inside
    /// the retained UI. The faithful GPU toolkit draws it from its own
    /// uploaded meshes (`meshes`, `players`' bodies); the modern renderer
    /// from the snapshot itself.
    pub fn frame_scene(
        &mut self,
        snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
        meshes: &crate::scene_meshes::SceneMeshes,
        players: Option<&crate::player_renderer::PlayersRenderer>,
        camera: &crate::render::OrbitCamera,
    ) -> anyhow::Result<()> {
        self.ensure_modern();
        if self.kind == RendererKind::Modern
            && !self.toolkit0()
            && !rs910_render_modern::modern_debug_flags::flags().check
        {
            let snapshot = rs910_core::profile::scope!(
                "scene snapshot",
                self.snapshots.capture(snapshot, self.snapshot_spare.take())
            );
            self.finish_render()?;
            let Backend::Modern(mut modern) =
                std::mem::replace(&mut self.backend, Backend::Pending)
            else {
                unreachable!("modern handoff");
            };
            modern.set_faithful_bloom(self.scene_bloom);
            if let Some(particles) = self.next_particles.take() {
                modern.set_particles(particles);
            }
            if let Some(shadows) = self.next_shadows.take() {
                modern.set_shadow_settings(shadows);
            }
            let composition = self.gpu.take_composition();
            self.enqueue(modern, composition, Some(snapshot));
            return Ok(());
        }
        self.finish_render()?;
        match &mut self.backend {
            Backend::Pending => unreachable!("completed renderer"),
            Backend::Gpu => {
                meshes.frame(&mut self.gpu, &mut self.device, snapshot, players, camera)
            }
            Backend::Null(null) => {
                null.frames += 1;
                Ok(())
            }
            Backend::Modern(modern) => {
                let check = rs910_render_modern::modern_debug_flags::flags().check;
                if check {
                    check_modern_draw_list(snapshot, meshes, players);
                }
                modern.set_faithful_bloom(self.scene_bloom);
                let frames = modern.frames();
                let mut prepared = self.gpu.scene_viewport().and_then(|viewport| {
                    let (width, height) = self.gpu.size();
                    modern.prepare_frame(
                        rs910_render_modern::frame::PrepareTarget {
                            device: &self.device.device,
                            queue: &self.device,
                            size: [width, height],
                            rect: viewport.rect,
                            clip: viewport.clip,
                        },
                        snapshot,
                    )
                });
                let mut submitted = false;
                let result = self
                    .gpu
                    .frame_composite_with_commands(&mut self.device, |target| {
                        let commands = prepared.take().map_or_else(Vec::new, |prepared| {
                            submitted = true;
                            modern.record_frame(
                                target.device,
                                target.encoder,
                                target.view,
                                prepared,
                            )
                        });
                        Ok(commands)
                    });
                if submitted {
                    modern.frame_submitted();
                } else if let Some(prepared) = prepared {
                    modern.frame_skipped(prepared);
                    // Installed cache writes survive the rollback. Drain them even
                    // while the surface stays hidden so copies cannot accumulate.
                    self.device.submit(std::iter::empty());
                }
                if check && modern.frames() != frames {
                    check_modern_sprites(snapshot, meshes, players, modern, self.scene_bloom);
                }
                result
            }
        }
    }

    /// Re-show the last completed modern canvas without preparing another scene.
    /// A new or resized canvas receives a full frame before it can be retained.
    /// The faithful and null backend paths keep their existing frame contract.
    pub fn present_scene(
        &mut self,
        snapshot: &crate::scene_snapshot::SceneSnapshot<'_>,
        meshes: &crate::scene_meshes::SceneMeshes,
        players: Option<&crate::player_renderer::PlayersRenderer>,
        camera: &crate::render::OrbitCamera,
    ) -> anyhow::Result<()> {
        self.finish_render()?;
        if matches!(self.backend, Backend::Modern(_)) {
            if let Some(composition) = self.gpu.take_present(&self.device) {
                let Backend::Modern(modern) =
                    std::mem::replace(&mut self.backend, Backend::Pending)
                else {
                    unreachable!();
                };
                self.enqueue(modern, composition, None);
                return Ok(());
            }
        }
        self.frame_scene(snapshot, meshes, players, camera)
    }

    /// The NXT renderer's sun shadow settings from the faithful
    /// `ClientOptions` values (renderer plan M3,
    /// `rs910_render_modern::shadows::Settings::from_options`); the other
    /// backends ignore them. It only reads the options: nothing CS2 or the
    /// packets see changes.
    pub fn set_modern_shadows(
        &mut self,
        scenery_shadows: i32,
        shadow_quality: i32,
        character_shadows: i32,
    ) {
        self.ensure_modern();
        let settings = rs910_render_modern::shadows::Settings::from_options(
            scenery_shadows,
            shadow_quality,
            character_shadows,
        );
        if let Backend::Modern(modern) = &mut self.backend {
            modern.set_shadow_settings(settings);
        } else if matches!(self.backend, Backend::Pending) {
            self.next_shadows = Some(settings);
        }
    }

    /// A loading-screen frame: the retained UI without a scene component
    /// (loading runs in toolkit 0; the faithful GPU toolkit draws it).
    pub fn frame_loading(
        &mut self,
        camera: &crate::render::OrbitCamera,
        env: &crate::env::EnvFrame,
    ) -> anyhow::Result<()> {
        self.render_barrier();
        self.gpu.set_threaded_composition(false);
        match &mut self.backend {
            // No scene: the faithful loading frame.
            Backend::Gpu | Backend::Modern(_) | Backend::Pending => {
                self.gpu
                    .frame_with_levels(&mut self.device, camera, env, &[], &[], &[])
            }
            Backend::Null(null) => {
                null.frames += 1;
                Ok(())
            }
        }
    }

    /// A `--screenshot` request (`Renderer::request_screenshot`); the null
    /// backend has no pixels, so it logs what it drew and counts the
    /// screenshot as written.
    pub fn request_screenshot(&mut self, path: std::path::PathBuf) {
        self.render_barrier();
        if let Backend::Null(null) = &mut self.backend {
            log::info!(
                "[client910] null renderer: {} frames, UI op digest {:016x}; no pixels, {} not written",
                null.frames,
                null.ui_digest,
                path.display()
            );
            null.screenshot = true;
            return;
        }
        self.device.request_screenshot(path);
    }
    /// `Renderer::reset_screenshot_written`.
    pub fn reset_screenshot_written(&mut self) {
        self.render_barrier();
        if let Backend::Null(null) = &mut self.backend {
            null.screenshot = false;
        }
        self.device.reset_screenshot_written();
    }
    /// `Renderer::screenshot_written`.
    pub fn screenshot_written(&self) -> bool {
        match &self.backend {
            Backend::Null(null) => null.screenshot,
            _ => self.device.screenshot_written(),
        }
    }

    /// The toolkit change's target rebuild on the device
    /// (`Renderer::recreate_toolkit_targets`). An `ActiveToolkit` method so a
    /// caller can read this toolkit's answers in the arguments
    /// (`bloom && renderer.supports_bloom()`).
    pub fn recreate_toolkit_targets(&mut self, samples: u32, bloom: bool) -> anyhow::Result<()> {
        self.render_barrier();
        self.gpu
            .recreate_toolkit_targets(&self.device, samples, bloom)?;
        self.scene_bloom = bloom;
        self.modern_follow_samples(samples);
        Ok(())
    }

    /// Renderer plan M8: the NXT backend's MSAA follows the
    /// `antiAliasing` level the faithful toolkit applies (865's
    /// antialiasing modes: level 0 is mode 1, FXAA without MSAA, the post
    /// chain's `fxaa`; a level with MSAA is mode 2 at the faithful sample
    /// count when the HDR target supports it, else 1x and FXAA). A change
    /// rebuilds only what depends on the count
    /// (`ModernRenderer::set_samples`); a backend not created yet takes the
    /// count when it is. Nothing the client reports changes.
    fn modern_follow_samples(&mut self, samples: u32) {
        if self.kind != RendererKind::Modern {
            return;
        }
        let (_, hdr) = self
            .device
            .scene_sample_counts(rs910_render_modern::frame::DEPTH_FORMAT);
        let want = if hdr.contains(&samples) { samples } else { 1 };
        if self.modern_samples != Some(want) && matches!(self.backend, Backend::Pending) {
            self.render_barrier();
        }
        self.modern_samples = Some(want);
        if matches!(self.backend, Backend::Gpu)
            && !self.answers.toolkit0
            && self.modern_startup.is_none()
        {
            // The first level: create the backend now, off the render
            // thread, with the pipeline sets of the other levels' counts
            // (the levels 0-2: 1, 2 and 4 samples, those the HDR target
            // supports), so neither its first frame nor a later
            // anti-aliasing change compiles.
            let more: Vec<u32> = [1, 2, 4].into_iter().filter(|n| hdr.contains(n)).collect();
            self.modern_startup = Some(rs910_render_modern::frame::startup::Startup::spawn(
                self.device.device.clone(),
                self.device.queue.clone(),
                self.device.config.format,
                want,
                &more,
                self.modern_settings,
            ));
            log::info!(
                "[client910] modern renderer: creating ({want}x MSAA) on its startup thread"
            );
        }
        if let Backend::Modern(modern) = &mut self.backend {
            if modern.samples() != want {
                let start = std::time::Instant::now();
                modern.set_samples(&self.device.device, want);
                log::info!(
                    "[client910] modern renderer: {want}x MSAA forward target (changed in {:.1} ms)",
                    start.elapsed().as_secs_f64() * 1000.0
                );
            }
        }
    }

    /// The faithful GPU toolkit with the device lent to it
    /// (`render::Faithful`): what the scene-mesh owners
    /// (`scene_meshes::SceneMeshes`, `player_renderer::PlayersRenderer`)
    /// upload through, at the client's upload points, whatever backend draws.
    pub fn faithful(&mut self) -> crate::render::Faithful<'_> {
        (&mut self.gpu, &self.device)
    }
    /// [`ActiveToolkit::faithful`] for the owners that only write existing
    /// buffers.
    pub fn faithful_ref(&self) -> crate::render::FaithfulRef<'_> {
        (&self.gpu, &self.device)
    }

    /// GPU adapter name (for startup logs).
    pub fn adapter_info(&self) -> String {
        self.device.adapter_info()
    }
    /// The adapter's identity for the console's `renderer` command.
    pub fn adapter_report(&self) -> crate::gpu_device::AdapterReport {
        self.device.adapter_report()
    }
    /// The device's allocated bytes
    /// (`gpu_device::Device::allocated_bytes`).
    pub fn allocated_bytes(&self) -> Option<u64> {
        self.device.allocated_bytes()
    }
    /// `Renderer::resize` of the surface and the faithful toolkit's targets.
    pub fn resize(&mut self, width: u32, height: u32) {
        self.render_barrier();
        self.gpu.resize(&mut self.device, width, height);
    }
    /// `Renderer::set_game_canvas`.
    pub fn set_game_canvas(
        &mut self,
        canvas: rs910_toolkit::game_canvas::Canvas,
    ) -> anyhow::Result<()> {
        if !self.gpu.game_canvas_matches(&self.device, canvas) {
            self.render_barrier();
        }
        self.gpu.set_game_canvas(&self.device, canvas)
    }
    /// `Renderer::console_sizes` (loads the developer console's fonts).
    pub fn console_sizes(&mut self, pack: &crate::cache::Pack) -> anyhow::Result<[i32; 2]> {
        self.gpu.console_sizes(&self.device, pack)
    }
    /// `Renderer::set_scene_effects` (scene samples and bloom).
    pub fn set_scene_effects(&mut self, count: u32, bloom: bool) -> anyhow::Result<()> {
        if !self.gpu.scene_effects_match(count, bloom) {
            self.render_barrier();
        }
        self.gpu.set_scene_effects(&self.device, count, bloom)?;
        self.scene_bloom = bloom;
        self.modern_follow_samples(count);
        Ok(())
    }
    /// `Renderer::prepare_particles`: this frame's GPU particle
    /// batches. The NXT backend draws the same frame (renderer plan M9,
    /// `rs910_render_modern::sprites::particles`).
    pub fn prepare_particles(
        &mut self,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        frame: &crate::particle_render::Frame,
    ) -> anyhow::Result<()> {
        self.ensure_modern();
        if self.kind == RendererKind::Modern && !self.toolkit0() {
            let particles = modern_particle_frame(frame);
            if let Backend::Modern(modern) = &mut self.backend {
                modern.set_particles(particles);
            } else {
                self.next_particles = Some(particles);
            }
            if !rs910_render_modern::modern_debug_flags::flags().check {
                return Ok(());
            }
        }
        self.gpu
            .prepare_particles(&self.device, pack, materials, frame)
    }
    /// `Renderer::prepare_skybox`: the faithful toolkit uploads this frame's
    /// sky whatever backend draws (its model-creation failures are the
    /// faithful answer the skybox build sees, like the capability
    /// profile); returns the boxes whose model it could not create.
    pub fn prepare_skybox(
        &mut self,
        assets: &crate::skybox_render::SkyAssets<'_>,
        sky: Option<rs910_scene::sky_frame::SkyFrame<'_>>,
        camera: &crate::camera::SceneCamera,
        env: &crate::env::EnvFrame,
    ) -> Vec<crate::skybox::SkyboxKey> {
        self.gpu
            .prepare_skybox(&self.device, assets, sky, camera, env)
    }

    /// The gate for the UI traversal's post-process capture. A capture opens
    /// only on the hardware toolkits: the software toolkit's capture is empty
    /// and its query false, so toolkit 0 draws no bloom, levels
    /// or colour remapping (`setBloom`/`setLevels`/`setColourRemapping` are
    /// no-ops there too), whatever draws it.
    pub fn postprocess_capture(&self) -> bool {
        !self.toolkit0() && self.gpu.postprocess_capture()
    }
    /// Whether bloom is supported (`Answers::supports_bloom`: false for
    /// toolkit 0).
    pub fn supports_bloom(&self) -> bool {
        self.answers.supports_bloom()
    }
    /// Whether anti-aliasing is supported (`Answers::supports_antialiasing`:
    /// false for toolkit 0).
    pub fn supports_antialiasing(&self) -> bool {
        self.answers.supports_antialiasing()
    }
    /// What the device's hardware toolkits answer, whichever toolkit is
    /// active now (the loading screens run toolkit 0 while the session
    /// installs the saved toolkit, so the installation reads this).
    pub fn hardware_answers(&self) -> rs910_toolkit::capability::Answers {
        rs910_toolkit::capability::Answers::hardware(self.answers.profile.clone())
    }
    /// Whether a toolkit creation with `count` scene samples succeeds (the
    /// capability profile; `Renderer::supports_scene_samples` over the same
    /// device set).
    pub fn supports_scene_samples(&self, count: u32) -> bool {
        self.answers.supports_scene_samples(count)
    }
    /// The GL texture format codes for the input telemetry (the
    /// capability profile).
    pub fn compressed_texture_formats(&self) -> &[i32] {
        self.answers.compressed_texture_formats()
    }
}

impl Drop for ActiveToolkit {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.render_barrier();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rs910_gpu_device::health::Fault;
    include!("active_toolkit/render_tests.rs");

    /// The CPU frame feeds modern even though its faithful GPU upload is
    /// skipped. Toolkit zero still accepts the same frame on the faithful path.
    #[test]
    #[ignore = "needs a GPU adapter and the cache"]
    fn modern_receives_nonzero_shared_particles_without_faithful_upload() -> anyhow::Result<()> {
        use crate::particle_render::{Batch, Frame, ParticleVertex, Segment};
        const DEVICE_SIZE: (u32, u32) = (64, 48);
        const UNTEXTURED: i32 = -1;
        const TRANSPARENT_LIST: u8 = 1;
        const QUAD_CORNERS: [[f32; 3]; 4] = [
            [-16., -16., 64.],
            [16., -16., 64.],
            [16., 16., 64.],
            [-16., 16., 64.],
        ];
        const TEX_COORDS: [[f32; 2]; 4] = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
        const COLOUR: [u8; 4] = [255, 128, 64, 255];
        let pack = crate::cache::Pack::open(rs910_core::test_support::pack_root());
        let materials = crate::texture::MaterialStore::load(&pack)?;
        let frame = Frame {
            vertices: QUAD_CORNERS
                .into_iter()
                .zip(TEX_COORDS)
                .map(|(pos, uv)| ParticleVertex {
                    pos,
                    colour: COLOUR,
                    uv,
                })
                .collect(),
            batches: vec![Batch {
                texture: UNTEXTURED,
                lit: false,
                first_vertex: 0,
                quads: 1,
            }],
            segments: vec![Segment {
                list: TRANSPARENT_LIST,
                entity: usize::MAX,
                mesh_end: usize::MAX,
                batches: 0..1,
            }],
            ..Default::default()
        };
        let mut toolkit =
            pollster::block_on(ActiveToolkit::headless(DEVICE_SIZE, RendererKind::Modern))?;
        toolkit.prepare_particles(&pack, &materials, &frame)?;
        let Backend::Modern(modern) = &toolkit.backend else {
            panic!("modern backend")
        };
        let received = modern.particles();
        assert_eq!(
            received.quads(),
            frame
                .batches
                .iter()
                .map(|batch| batch.quads as usize)
                .sum::<usize>()
        );
        assert!(!received.vertices.is_empty());
        assert_eq!(
            bytemuck::cast_slice::<_, u8>(&received.vertices),
            bytemuck::cast_slice::<_, u8>(&frame.vertices),
            "same ordered quad bytes"
        );
        assert_eq!(received.batches[0].texture, frame.batches[0].texture);
        assert_eq!(received.segments[0].list, frame.segments[0].list);
        assert_eq!(received.segments[0].entity, frame.segments[0].entity);
        assert_eq!(received.segments[0].batches, frame.segments[0].batches);
        toolkit.set_toolkit0(true);
        toolkit.prepare_particles(&pack, &materials, &frame)?;
        toolkit.wait_idle();
        assert_eq!(toolkit.device.health.validation_errors(), 0);
        Ok(())
    }

    /// The device is lost twice in a row (a driver reset, then the new device
    /// failing too): each loss is reported once, the toolkit recovers on a
    /// new device with new resources that work, the modern renderer is made
    /// again over the new device and, the second time, replaced by the
    /// faithful one. The window scale and toolkit 0's answers carry over.
    #[test]
    #[ignore = "needs a GPU adapter"]
    fn a_lost_device_is_recovered_with_new_resources() -> anyhow::Result<()> {
        let mut toolkit =
            pollster::block_on(ActiveToolkit::headless((64, 48), RendererKind::Modern))?;
        toolkit.set_ui_scale(2.0);
        assert_eq!(toolkit.take_fault(), None);
        let first = Arc::as_ptr(&toolkit.device.device);

        toolkit.lose_device();
        let fault = toolkit.take_fault().expect("the loss is reported");
        assert!(
            matches!(&fault, Fault::Lost { reason, .. } if reason == "Destroyed"),
            "{fault:?}"
        );
        assert_eq!(toolkit.take_fault(), None, "once");
        toolkit.recover(Recovery::Recreate)?;
        assert_ne!(Arc::as_ptr(&toolkit.device.device), first, "a new device");
        assert!(!toolkit.device.health.is_lost());
        assert_eq!(toolkit.take_fault(), None);
        assert_eq!(toolkit.device_generation(), 1);
        assert_eq!(
            toolkit.canvas_size(),
            (32, 24),
            "the window scale carried over"
        );
        assert!(!toolkit.toolkit0());

        // The modern renderer is made over the new device when the next scene
        // frame asks for it, and the new device takes work without errors.
        toolkit.ensure_modern();
        assert!(matches!(toolkit.backend, Backend::Modern(_)));
        toolkit.device.submit([]);
        toolkit.wait_idle();
        assert_eq!(toolkit.device.health.validation_errors(), 0);
        assert_eq!(toolkit.take_fault(), None);

        // Lost again: the safer renderer.
        toolkit.lose_device();
        assert!(toolkit.take_fault().is_some());
        toolkit.recover(Recovery::SaferRenderer)?;
        assert_eq!(toolkit.kind(), RendererKind::FaithfulGpu);
        toolkit.ensure_modern();
        assert!(
            matches!(toolkit.backend, Backend::Gpu),
            "no modern backend any more"
        );
        assert_eq!(toolkit.device_generation(), 2);
        toolkit.device.submit([]);
        toolkit.wait_idle();
        assert_eq!(toolkit.device.health.validation_errors(), 0);
        Ok(())
    }
}
