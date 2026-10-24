//! The shell's active toolkit (selected and recreated on preference
//! changes): the device layer (`rs910_gpu_device::gpu_device::Device`), the
//! 2D/UI toolkit (`rs910_render_gpu::render::Renderer`) and the [`Backend`]
//! that draws the scene: the modern renderer (`rs910-render-modern`), or
//! nothing (`--renderer null`, headless tests and tooling).
//!
//! The shell owns the `Device` and lends it per call (`&Device` to upload,
//! `&mut Device` to present). The modern renderer draws
//! [`ActiveToolkit::frame_scene`]'s renderer-neutral `SceneSnapshot` (the
//! backend contract is in `rs910_scene::scene_snapshot`'s module docs) on
//! the render thread, into the scene viewport of the toolkit's composition
//! (`Renderer::take_composition`: the UI below and over it, the console,
//! the retained frame, the canvas scale-up, screenshots). Loading and
//! message-box frames are the toolkit's alone. Methods that branch on the
//! backend dispatch here; everything else derefs to the `Renderer`.
//!
//! The toolkit id (`displayMode`) is game state: toolkit 0 answers as the
//! software toolkit (no bloom, no anti-aliasing, no post-process capture;
//! `rs910_toolkit::capability::Answers`), and every toolkit draws through
//! the GPU. Whatever draws, every capability answer comes from the device's
//! profile ([`RendererKind::capability_profile`],
//! `rs910_toolkit::capability::Profile`), so the UI, CS2 and `ClientOptions`
//! see the same answers. The shell hands the modern renderer the frame's
//! CPU particle quads ([`ActiveToolkit::prepare_particles`]: the particle
//! owners are shell state the snapshot does not carry) and the game's bloom
//! setting (billboards hide under it).

mod render_thread;

use std::sync::Arc;

use winit::window::Window;

use rs910_render_gpu::render::Renderer;
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
        plan: rs910_toolkit::ui_paint::Plan,
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

/// What draws the scene (`--renderer`), independently of the legacy toolkit
/// preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum RendererKind {
    /// Nothing is drawn or presented (the UI's frames are digested); the
    /// device still exists for the capability answers.
    Null,
    /// The modern scene renderer (`rs910-render-modern`). The default.
    #[default]
    #[value(alias = "nxt")]
    Modern,
}

impl RendererKind {
    /// The capability answers the game sees, given the device's (`device`,
    /// from `Renderer::capability_profile`): every renderer answers with the
    /// device profile, so the saved `ClientOptions`, the scripts' graphics
    /// settings and the anti-aliasing fallback never depend on it.
    pub fn capability_profile(
        self,
        device: rs910_toolkit::capability::Profile,
    ) -> rs910_toolkit::capability::Profile {
        match self {
            RendererKind::Null | RendererKind::Modern => device,
        }
    }
}

/// What draws the scene.
pub enum Backend {
    /// The modern renderer is not created yet (it is made on first use).
    Unstarted,
    /// `--renderer null`: nothing is drawn or presented.
    Null(NullFrames),
    /// The modern renderer, on the main thread between frames.
    Modern(Box<rs910_render_modern::frame::ModernRenderer>),
    /// The modern renderer and surface are owned by the render thread.
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
    /// The device layer the backends share: the window surface,
    /// device and queue, lent to the backend that draws each call.
    device: rs910_gpu_device::gpu_device::Device,
    backend: Backend,
    /// `--renderer`.
    kind: RendererKind,
    /// The active toolkit's capability answers: whether toolkit 0
    /// (`displayMode` 0) is active, and the hardware toolkit's profile
    /// ([`RendererKind::capability_profile`]).
    answers: rs910_toolkit::capability::Answers,
    /// The game's bloom setting (`Renderer::set_scene_effects`' `bloom`
    /// once it succeeded): the modern renderer's billboard set and bloom
    /// default follow it.
    scene_bloom: bool,
    /// The modern renderer's forward-target sample count (the anti-aliasing
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
    /// The far plane (fine units) the last modern frame drew its far scene
    /// to; `None` when it drew only the classic window.
    scene_far_plane: Option<i32>,
    /// The dynamic locs the last frame's far scene wanted posed.
    scene_far_dynamic: Vec<usize>,
    /// The window the device draws to, kept to make the surface again after
    /// a device loss (none for a headless toolkit).
    window: Option<Arc<Window>>,
    /// How many devices this toolkit has had after the first.
    generation: u32,
    render_thread: Option<render_thread::RenderThread>,
    presentation_rate: render_thread::PresentationRate,
    occluded: bool,
    snapshots: rs910_scene::scene_snapshot::owned::SnapshotBuilder,
    snapshot_spare: Option<rs910_scene::scene_snapshot::owned::OwnedSnapshot>,
    next_particles: Option<rs910_render_modern::sprites::particles::ParticleFrame>,
    next_shadows: Option<rs910_render_modern::shadows::Settings>,
    frame_id: u64,
    cycle_id: i32,
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
        let capture_requested = self.device.pending_screenshot.is_some();
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
                presented: None,
                occluded: !capture_requested
                    && (self.occluded
                        || self
                            .window
                            .as_ref()
                            .is_some_and(|window| window.is_minimized().unwrap_or(false))),
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
        if let Some(presented) = job.presented {
            self.presentation_rate.record(presented);
        }
        self.device = job.device;
        self.scene_far_plane = job.modern.far_plane().map(|plane| plane as i32);
        self.scene_far_dynamic.clear();
        self.scene_far_dynamic
            .extend_from_slice(job.modern.far_dynamic());
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
    ) -> anyhow::Result<rs910_gpu_device::gpu_device::Device> {
        // The modern renderer's material textures use BC or ETC2 blocks
        // when the adapter has them.
        let optional = if kind == RendererKind::Modern {
            rs910_render_modern::device_features::optional_device_features()
        } else {
            Default::default()
        };
        match target {
            Some(target) => {
                rs910_gpu_device::gpu_device::Device::new_with_features(
                    target,
                    size,
                    Renderer::device_options(),
                    optional,
                )
                .await
            }
            None => {
                rs910_gpu_device::gpu_device::Device::headless(
                    size,
                    Renderer::device_options(),
                    optional,
                )
                .await
            }
        }
    }

    /// The toolkit over `device`: the GPU renderer and the hardware backend of
    /// `kind` until the session installs its toolkit.
    fn over(device: rs910_gpu_device::gpu_device::Device, kind: RendererKind) -> Self {
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
            scene_far_plane: None,
            scene_far_dynamic: Vec::new(),
            modern_startup: None,
            window: None,
            generation: 0,
            render_thread: None,
            presentation_rate: Default::default(),
            occluded: false,
            snapshots: Default::default(),
            snapshot_spare: None,
            next_particles: None,
            next_shadows: None,
            frame_id: 0,
            cycle_id: 0,
        };
        // The modern renderer is created once, with the session's
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

    /// The selected scene renderer.
    pub fn kind(&self) -> RendererKind {
        self.kind
    }

    /// Surface visibility only controls GPU preparation, never game logic.
    pub fn set_occluded(&mut self, occluded: bool) {
        self.occluded = occluded;
    }

    /// Successful modern scene presentations per second, measured on the worker.
    pub fn presentation_fps(&self) -> f32 {
        self.presentation_rate.fps()
    }

    /// Answer a device fault: make a new device and surface, and a new
    /// toolkit over them, as a toolkit change does. Everything created on the
    /// old device is dropped; the owners of uploads (the players, the sky,
    /// the minimap base) must be reset by the caller, which rebuilds the
    /// scene. The window's scale and the saved render
    /// scale carry over; the toolkit 0 answers follow what was active.
    ///
    /// On an error nothing changed except that the modern backend, if it
    /// was drawing, is gone.
    pub fn recover(&mut self) -> anyhow::Result<()> {
        self.render_barrier();
        // A modern backend still being made on its start-up thread is not
        // wanted: it was made for the old device.
        self.gpu.set_threaded_composition(false);
        drop(self.modern_startup.take());
        self.backend = Backend::Unstarted;
        let target = self.window.clone();
        pollster::block_on(self.device.recreate(target))?;
        self.gpu = Renderer::new(&self.device);
        self.presentation_rate = Default::default();
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
        self.recover()?;
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
    /// device supports it for the HDR format (    /// `Device::scene_sample_counts`); its quality settings are the defaults
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

    /// The far plane (fine units) the last scene frame drew to beyond the
    /// classic one, while the modern far scene draws; the ground pick reaches
    /// it.
    pub fn scene_far_plane(&self) -> Option<i32> {
        self.scene_far_plane
    }

    /// The dynamic locs past the classic plan the last frame's far scene
    /// shows (live entity ids of the scene it drew).
    pub fn scene_far_dynamic(&self) -> &[usize] {
        &self.scene_far_dynamic
    }

    pub fn modern_controls_available(&self) -> bool {
        self.kind == RendererKind::Modern
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

    /// Create the selected modern backend on first use, independently of
    /// the legacy toolkit preference.
    fn ensure_modern(&mut self) {
        if self.kind == RendererKind::Modern && matches!(self.backend, Backend::Unstarted) {
            self.backend = self.new_modern();
        }
    }

    /// The initial backend for the selected scene renderer.
    fn hardware_backend(kind: RendererKind) -> Backend {
        match kind {
            RendererKind::Null => Backend::Null(NullFrames::default()),
            // Created by `ensure_modern`/`set_toolkit0` over the device.
            RendererKind::Modern => Backend::Unstarted,
        }
    }

    /// The legacy toolkit preference changes capability answers, independently
    /// of the selected scene renderer. Modern stays on the native GPU device.
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
        self.ensure_modern();
    }
    /// Toolkit 0 (the software toolkit) is the active toolkit, whatever `--renderer` draws with.
    pub fn toolkit0(&self) -> bool {
        self.answers.toolkit0
    }

    pub fn prepare_ui(
        &mut self,
        output: rs910_toolkit::ui_output::Output<rs910_scene::interface_model::Draw>,
    ) -> anyhow::Result<()> {
        match &mut self.backend {
            Backend::Null(null) => {
                null.ui_digest = rs910_toolkit::toolkit::digest(&output.recording.ops);
                return Ok(());
            }
            Backend::Unstarted | Backend::Modern(_) | Backend::Pending => {}
        }
        self.ensure_modern();
        self.gpu
            .set_threaded_composition(self.kind == RendererKind::Modern);
        self.gpu.prepare_ui(&self.device, output)
    }

    /// `MessageBox.draw(text, true, toolkit, ..)` in a rebuild state
    /// (`Renderer::frame_message_box`), over the retained last frame.
    pub fn frame_message_box(&mut self, plan: rs910_toolkit::ui_paint::Plan) -> anyhow::Result<()> {
        self.frame_message_box_over(plan, Backdrop::LastFrame)
    }

    /// [`Self::frame_message_box`] over `backdrop`.
    pub fn frame_message_box_over(
        &mut self,
        plan: rs910_toolkit::ui_paint::Plan,
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
    /// benchmark (the modern renderer through its own passes, `--renderer
    /// null` through the device benchmark).
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
            RendererKind::Null => rs910_gpu_device::ui_preferences_metric_gpu::measure(
                &self.device.device,
                &self.device.queue,
                request,
            ),
        }
    }

    /// The minimap base rebuild (`Renderer::render_minimap_base`).
    pub fn render_minimap_base(
        &mut self,
        plan: &rs910_scene::minimap::BasePlan,
        world: rs910_render_gpu::render::MinimapWorld<'_>,
        env: &rs910_scene::env::EnvFrame,
    ) -> anyhow::Result<u64> {
        self.render_barrier();
        if let Backend::Null(_) = self.backend {
            // Nothing draws: the base sprite is only an id.
            let id = *self.gpu.next_external_id();
            *self.gpu.next_external_id() += 1;
            return Ok(id);
        }
        self.gpu
            .render_minimap_base(&mut self.device, plan, world, env)
    }

    /// Drop a base sprite.
    pub fn release_minimap_base(&mut self, id: u64) {
        self.render_barrier();
        self.gpu.release_minimap_base(id);
    }

    pub fn prepare_console(
        &mut self,
        console: &dyn rs910_toolkit::console_draw::ConsoleView,
        cycle: i32,
        focused: bool,
    ) -> anyhow::Result<()> {
        if let Backend::Null(_) = self.backend {
            return Ok(());
        }
        self.gpu
            .prepare_console(&self.device, console, cycle, focused)
    }

    /// The scene draw and the frame: the modern renderer draws this frame's
    /// renderer-neutral scene (`snapshot`) on the render thread, inside the
    /// retained UI; the null backend counts it.
    pub fn frame_scene(
        &mut self,
        snapshot: &rs910_scene::scene_snapshot::SceneSnapshot<'_>,
    ) -> anyhow::Result<()> {
        self.ensure_modern();
        if self.kind != RendererKind::Modern {
            if let Backend::Null(null) = &mut self.backend {
                null.frames += 1;
            }
            return Ok(());
        }
        let snapshot = rs910_core::profile::scope!(
            "scene snapshot",
            self.snapshots.capture(snapshot, self.snapshot_spare.take())
        );
        self.finish_render()?;
        let Backend::Modern(mut modern) = std::mem::replace(&mut self.backend, Backend::Pending)
        else {
            unreachable!("modern handoff");
        };
        modern.set_scene_bloom(self.scene_bloom);
        if let Some(particles) = self.next_particles.take() {
            modern.set_particles(particles);
        }
        if let Some(shadows) = self.next_shadows.take() {
            modern.set_shadow_settings(shadows);
        }
        let composition = self.gpu.take_composition();
        self.enqueue(modern, composition, Some(snapshot));
        Ok(())
    }

    /// Render a fresh scene from the last authoritative state and frozen UI.
    /// Presentation never repeats game logic, UI hooks, or actor write-back.
    pub fn present_scene(
        &mut self,
        snapshot: &rs910_scene::scene_snapshot::SceneSnapshot<'_>,
    ) -> anyhow::Result<()> {
        self.ensure_modern();
        self.finish_render()?;
        if self.kind == RendererKind::Modern {
            if let Some(mut owned) = self.snapshot_spare.take() {
                owned.update_presentation(snapshot);
                self.gpu.reuse_composition();
                let Backend::Modern(modern) =
                    std::mem::replace(&mut self.backend, Backend::Pending)
                else {
                    unreachable!("completed modern presentation");
                };
                let composition = self.gpu.take_composition();
                self.enqueue(modern, composition, Some(owned));
                return Ok(());
            }
        }
        self.frame_scene(snapshot)
    }

    /// The modern renderer's sun shadow settings from the `ClientOptions`
    /// values (`rs910_render_modern::shadows::Settings::from_options`); the
    /// null backend ignores them. It only reads the options: nothing CS2 or the
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

    /// A loading-screen frame: the retained UI with an empty scene viewport
    /// cleared to the fog colour.
    pub fn frame_loading(&mut self, env: &rs910_scene::env::EnvFrame) -> anyhow::Result<()> {
        self.render_barrier();
        self.gpu.set_threaded_composition(false);
        if let Backend::Null(null) = &mut self.backend {
            null.frames += 1;
            return Ok(());
        }
        self.gpu.frame_loading(&mut self.device, env.clear)
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

    /// The modern renderer's MSAA follows the `antiAliasing` level (level 0
    /// is FXAA without MSAA; a level with MSAA uses its sample count when the
    /// HDR target supports it, else 1x and FXAA). A change
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
        if matches!(self.backend, Backend::Unstarted) && self.modern_startup.is_none() {
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

    /// GPU adapter name (for startup logs).
    pub fn adapter_info(&self) -> String {
        self.device.adapter_info()
    }
    /// The adapter's identity for the console's `renderer` command.
    pub fn adapter_report(&self) -> rs910_gpu_device::gpu_device::AdapterReport {
        self.device.adapter_report()
    }
    /// The device's allocated bytes
    /// (`gpu_device::Device::allocated_bytes`).
    pub fn allocated_bytes(&self) -> Option<u64> {
        self.device.allocated_bytes()
    }
    /// `Renderer::resize` of the surface and the toolkit's targets.
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
    pub fn console_sizes(&mut self, pack: &rs910_js5::cache::Pack) -> anyhow::Result<[i32; 2]> {
        self.gpu.console_sizes(&self.device, pack)
    }
    /// `Renderer::set_scene_effects` (scene samples and bloom).
    pub fn set_scene_effects(&mut self, count: u32, bloom: bool) -> anyhow::Result<()> {
        if !self.gpu.scene_effects_match(count, bloom) {
            self.render_barrier();
        }
        self.gpu.set_scene_effects(count, bloom)?;
        self.scene_bloom = bloom;
        self.modern_follow_samples(count);
        Ok(())
    }
    /// This frame's particle quads (`rs910_scene::particle_quads`), for the
    /// modern renderer's next frame.
    pub fn prepare_particles(
        &mut self,
        particles: rs910_render_modern::sprites::particles::ParticleFrame,
    ) {
        self.ensure_modern();
        match &mut self.backend {
            Backend::Modern(modern) => modern.set_particles(particles),
            Backend::Pending => self.next_particles = Some(particles),
            Backend::Unstarted | Backend::Null(_) => {}
        }
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

    /// The device is lost twice in a row (a driver reset, then the new device
    /// failing too): each loss is reported once, the toolkit recovers on a
    /// new device with new resources that work, and the modern renderer is
    /// made again over the new device each time. The window scale and toolkit 0's answers carry over.
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
        toolkit.recover()?;
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

        // Lost again: recovered the same way.
        toolkit.lose_device();
        assert!(toolkit.take_fault().is_some());
        toolkit.recover()?;
        toolkit.ensure_modern();
        assert!(matches!(toolkit.backend, Backend::Modern(_)));
        assert_eq!(toolkit.device_generation(), 2);
        toolkit.device.submit([]);
        toolkit.wait_idle();
        assert_eq!(toolkit.device.health.validation_errors(), 0);
        Ok(())
    }
}
