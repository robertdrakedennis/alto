//! The GPU device layer (code-quality programme Phase 3.4, NXT renderer
//! plan A2; target-architecture §4 `device.rs`): the wgpu instance, adapter,
//! device, queue and window surface the renderers share, the capability
//! answers read from them and screenshot readback.
//!
//! The shell (`client910::active_toolkit::ActiveToolkit`) owns the one
//! [`Device`] and lends it to the backend that draws each call (programme
//! Phase 4.1): the faithful GPU toolkit (`rs910_render_gpu::render::Renderer`
//! takes it as its `gpu` argument) and the modern renderer
//! (`rs910-render-modern`, plan M1). The shell creates it for its
//! window: [`Device::new`] takes any `wgpu::SurfaceTarget`, so this layer
//! names no windowing crate. The device's own answers live here too: the
//! adapter name and features, the allocated bytes
//! and the screenshot request state.
//!
//! [`Device::new`] is the device half of the former `Renderer::new`
//! (toolkit creation) and [`Device::scene_sample_counts`] its capability
//! half, with the old expressions in the old order; `client_watch_gpu` and
//! `ui_preferences_metric_gpu` are the device answers the UI asks for
//! (the input telemetry's texture formats, the performance metric).

use std::collections::HashSet;

/// The profiling switches device creation reads (the GPU renderer's
/// diagnostics `CLIENT910_PROFILE` and `CLIENT910_PROFILE_PRESENT`;
/// `Renderer::device_options`).
#[derive(Clone, Copy, Debug, Default)]
pub struct DeviceOptions {
    /// `FrameProfile::enabled()`: request `TIMESTAMP_QUERY` when the adapter
    /// has it and log the present mode.
    pub profile: bool,
    /// `CLIENT910_PROFILE_PRESENT=immediate`: present immediately while
    /// profiling when the surface supports it.
    pub present_immediate: bool,
    /// The engine profiler is compiled in (`rs910_core::profile::COMPILED`):
    /// request `TIMESTAMP_QUERY` when the adapter has it, for its per-pass
    /// GPU times (nothing else of `profile`).
    pub timestamps: bool,
}

/// The wgpu context of the client window (the toolkit's device state).
pub struct Device {
    // Kept per contract (surface creation borrows instance internals).
    pub instance: wgpu::Instance,
    pub adapter: wgpu::Adapter,
    /// Shared (`Arc`) so a renderer can be created on a background thread
    /// while the login screens show (the modern renderer's start-up,
    /// `rs910_render_modern::frame::startup`); every other user borrows it.
    pub device: std::sync::Arc<wgpu::Device>,
    pub queue: std::sync::Arc<wgpu::Queue>,
    /// The window surface; none for a headless device (tests, tools), which
    /// draws nothing to a screen.
    pub surface: Option<wgpu::Surface<'static>>,
    pub config: wgpu::SurfaceConfiguration,
    /// What the device reports about itself: loss and errors
    /// ([`crate::health`]).
    pub health: std::sync::Arc<crate::health::Health>,
    /// How the device was asked for, kept to ask again after a loss.
    options: DeviceOptions,
    optional: wgpu::Features,
    /// Next multi-level frame is read back and written here as PNG.
    pub pending_screenshot: Option<std::path::PathBuf>,
    /// Set once a requested screenshot has been written.
    pub screenshot_written: bool,
    /// The buffer writes since the last [`Device::submit`]
    /// ([`crate::uploads`]).
    uploads: std::sync::Arc<std::sync::Mutex<crate::uploads::Uploads>>,
}

/// Buffer writes through the device's staging belt, submitted ahead of the
/// next [`Device::submit`] (`crate::uploads` module docs).
impl crate::uploads::Uploader for Device {
    fn write_buffer(&self, buffer: &wgpu::Buffer, offset: u64, data: &[u8]) {
        self.uploads
            .lock()
            .expect("uploads lock")
            .write(&self.device, buffer, offset, data);
    }
    fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }
    fn submit_uploads(&self, commands: Vec<wgpu::CommandBuffer>) -> wgpu::SubmissionIndex {
        self.submit(commands)
    }
}

impl Device {
    /// A surface-free view for main-thread data preparation while the render
    /// thread owns presentation. It shares ordered uploads, not mutable surface state.
    pub fn upload_context(&self) -> Self {
        Self {
            instance: self.instance.clone(),
            adapter: self.adapter.clone(),
            device: self.device.clone(),
            queue: self.queue.clone(),
            surface: None,
            config: self.config.clone(),
            health: self.health.clone(),
            options: self.options,
            optional: self.optional,
            pending_screenshot: None,
            screenshot_written: self.screenshot_written,
            uploads: self.uploads.clone(),
        }
    }

    /// Create the GPU context for `target` (the shell's window, `size` its
    /// inner size in physical pixels) and configure the surface.
    pub async fn new(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
        options: DeviceOptions,
    ) -> anyhow::Result<Self> {
        Self::new_with_features(target, size, options, wgpu::Features::empty()).await
    }

    /// [`Device::new`], also requesting those of `optional` the adapter
    /// has. The NXT renderer asks for the block-compressed texture features
    /// (`TEXTURE_COMPRESSION_BC`/`_ETC2`, renderer plan M2); the faithful
    /// renderers ask for none, so their device is `Device::new`'s.
    /// the input telemetry's texture formats read the adapter's features either
    /// way ([`Device::adapter_features`]).
    pub async fn new_with_features(
        target: impl Into<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
        options: DeviceOptions,
        optional: wgpu::Features,
    ) -> anyhow::Result<Self> {
        Self::create(Some(target.into()), size, options, optional).await
    }

    /// A device with no window: what draws to it goes nowhere. For tests and
    /// tools that need the renderers' resources and not a screen.
    pub async fn headless(
        size: (u32, u32),
        options: DeviceOptions,
        optional: wgpu::Features,
    ) -> anyhow::Result<Self> {
        Self::create(None, size, options, optional).await
    }

    /// Replace the device, queue and surface with new ones, as after a lost
    /// device: `target` is the window again (none for a headless device).
    /// The surface keeps its size and options; whatever was created on the
    /// old device is useless and must be created again. The old device
    /// is destroyed (it may already be lost). When the new device cannot be
    /// made the error comes back and the device has no surface.
    pub async fn recreate(
        &mut self,
        target: Option<impl Into<wgpu::SurfaceTarget<'static>>>,
    ) -> anyhow::Result<()> {
        let size = (self.config.width, self.config.height);
        // The window has one surface at a time: the old one goes first.
        drop(self.surface.take());
        let fresh = Self::create(target.map(Into::into), size, self.options, self.optional).await?;
        self.device.destroy();
        *self = fresh;
        Ok(())
    }

    async fn create(
        target: Option<wgpu::SurfaceTarget<'static>>,
        size: (u32, u32),
        options: DeviceOptions,
        optional: wgpu::Features,
    ) -> anyhow::Result<Self> {
        let (width, height) = (size.0.max(1), size.1.max(1));

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let surface = target
            .map(|target| {
                instance
                    .create_surface(target)
                    .map_err(|err| anyhow::anyhow!("create wgpu surface: {err}"))
            })
            .transpose()?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: surface.as_ref(),
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .map_err(|err| anyhow::anyhow!("no suitable GPU adapter found: {err}"))?;
        let optional = if adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::INDIRECT_EXECUTION)
        {
            optional
        } else {
            optional
                - (wgpu::Features::INDIRECT_FIRST_INSTANCE
                    | wgpu::Features::MULTI_DRAW_INDIRECT_COUNT)
        };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("client910"),
                required_features: (adapter.features()
                    & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES)
                    | if options.profile || options.timestamps {
                        adapter.features() & wgpu::Features::TIMESTAMP_QUERY
                    } else {
                        wgpu::Features::empty()
                    }
                    | (adapter.features() & optional),
                required_limits: renderer_limits(&adapter),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                memory_hints: Default::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|err| anyhow::anyhow!("request wgpu device: {err}"))?;
        let health = crate::health::Health::watch(&device);

        let caps = surface
            .as_ref()
            .map(|surface| surface.get_capabilities(&adapter));
        // Non-sRGB surface: the original client writes shader output straight to
        // the framebuffer with no gamma stage (its 0.7 texture gamma and the
        // 0.7 colour LUTs are the only "gamma" anywhere), so an sRGB encode
        // on store would brighten every floor/model against the reference.
        let format = match &caps {
            Some(caps) => caps
                .formats
                .iter()
                .copied()
                .find(|f| !f.is_srgb())
                .or_else(|| caps.formats.first().copied())
                .ok_or_else(|| anyhow::anyhow!("surface exposes no texture formats"))?,
            None => wgpu::TextureFormat::Bgra8Unorm,
        };
        let present_modes = caps.as_ref().map(|caps| caps.present_modes.as_slice());
        let config = wgpu::SurfaceConfiguration {
            // COPY_SRC so `request_screenshot` can read the presented frame back.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | caps.as_ref().map_or(wgpu::TextureUsages::COPY_DST, |caps| {
                    caps.usages & wgpu::TextureUsages::COPY_DST
                }),
            format,
            width,
            height,
            present_mode: if options.profile
                && options.present_immediate
                && present_modes.is_some_and(|m| m.contains(&wgpu::PresentMode::Immediate))
            {
                wgpu::PresentMode::Immediate
            } else {
                present_modes
                    .and_then(|m| m.first().copied())
                    .unwrap_or(wgpu::PresentMode::Fifo)
            },
            alpha_mode: caps
                .as_ref()
                .and_then(|caps| caps.alpha_modes.first().copied())
                .unwrap_or(wgpu::CompositeAlphaMode::Auto),
            view_formats: Vec::new(),
            desired_maximum_frame_latency: 2,
            color_space: wgpu::SurfaceColorSpace::Auto,
        };
        if options.profile {
            log::info!(
                "[perf] present={:?} supported={:?} latency={} adapter={:?}",
                config.present_mode,
                present_modes,
                config.desired_maximum_frame_latency,
                adapter.get_info()
            );
        }
        if let Some(surface) = &surface {
            surface.configure(&device, &config);
        }
        let uploads = std::sync::Arc::new(std::sync::Mutex::new(crate::uploads::Uploads::new(
            device.clone(),
        )));
        Ok(Self {
            instance,
            adapter,
            device: std::sync::Arc::new(device),
            queue: std::sync::Arc::new(queue),
            surface,
            config,
            health,
            options,
            optional,
            pending_screenshot: None,
            screenshot_written: false,
            uploads,
        })
    }

    /// Configure the surface for the current `config` (after a resize or a
    /// lost surface); nothing for a headless device.
    pub fn configure_surface(&self) {
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
    }

    /// The next frame's texture. A headless device has no screen: its
    /// frames are skipped like those of a hidden window.
    pub fn acquire(&self) -> wgpu::CurrentSurfaceTexture {
        match &self.surface {
            Some(surface) => surface.get_current_texture(),
            None => wgpu::CurrentSurfaceTexture::Occluded,
        }
    }

    /// `queue.submit(commands)` preceded by the buffer writes staged since
    /// the last submission (`Queue::write_buffer` semantics, `crate::uploads`).
    /// Every submission of either renderer goes through here.
    pub fn submit<I: IntoIterator<Item = wgpu::CommandBuffer>>(
        &self,
        commands: I,
    ) -> wgpu::SubmissionIndex {
        let mut uploads = self.uploads.lock().expect("uploads lock");
        let staged = uploads.take();
        let index = self.queue.submit(staged.into_iter().chain(commands));
        uploads.recall();
        index
    }

    #[cfg(test)]
    pub(crate) fn has_pending_uploads(&self) -> bool {
        self.uploads.lock().expect("uploads lock").has_pending()
    }

    /// The scene sample counts the surface colour format and `depth` support
    /// (1 is always available) and the subset the HDR (bloom) scene target
    /// supports: the answers behind anti-aliasing and bloom support on this
    /// device.
    pub fn scene_sample_counts(&self, depth: wgpu::TextureFormat) -> (HashSet<u32>, HashSet<u32>) {
        let colour_features = self.adapter.get_texture_format_features(self.config.format);
        let depth_features = self.adapter.get_texture_format_features(depth);
        // Scene sample counts the colour + depth formats support
        // 1 is always available.
        let mut msaa_samples = std::collections::HashSet::from([1]);
        for count in [2, 4] {
            if colour_features.flags.sample_count_supported(count)
                && depth_features.flags.sample_count_supported(count)
                && (count == 4
                    || self
                        .device
                        .features()
                        .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES))
            {
                msaa_samples.insert(count);
            }
        }
        // Bloom renders the scene into an `Rgba16Float` target.
        let hdr_features = self
            .adapter
            .get_texture_format_features(wgpu::TextureFormat::Rgba16Float);
        let hdr_samples: std::collections::HashSet<u32> = [1, 2, 4]
            .into_iter()
            .filter(|&count| {
                msaa_samples.contains(&count) && hdr_features.flags.sample_count_supported(count)
            })
            .collect();
        (msaa_samples, hdr_samples)
    }

    /// GPU adapter name (for startup logs).
    pub fn adapter_info(&self) -> String {
        self.adapter.get_info().name
    }

    /// What the adapter reports about itself, for the developer console's
    /// `renderer` command.
    pub fn adapter_report(&self) -> AdapterReport {
        AdapterReport::from_info(&self.adapter.get_info())
    }

    /// Adapter feature set (input telemetry texture-format report).
    pub fn adapter_features(&self) -> wgpu::Features {
        self.adapter.features()
    }

    /// The next fault to answer ([`crate::health::Health::take_fault`]).
    /// The device is polled first: a loss reaches its callback when the
    /// device is polled, not at the moment it happens.
    pub fn take_fault(&self) -> Option<crate::health::Fault> {
        let _ = self.device.poll(wgpu::PollType::Poll);
        self.health.take_fault()
    }

    /// The bytes of the buffers and textures the toolkit holds
    /// ([`allocated_bytes`] of the device).
    #[must_use]
    pub fn allocated_bytes(&self) -> Option<u64> {
        allocated_bytes(&self.device)
    }
    /// Read the next presented frame back and write it as PNG (headless
    /// verification without a display).
    pub fn request_screenshot(&mut self, path: std::path::PathBuf) {
        self.pending_screenshot = Some(path);
        self.screenshot_written = false;
    }

    /// Forget a written screenshot (loading-frame diagnostics) so the next
    /// `--screenshot` request decides the exit.
    pub fn reset_screenshot_written(&mut self) {
        self.screenshot_written = false;
    }

    /// True once the requested screenshot has been written.
    pub fn screenshot_written(&self) -> bool {
        self.screenshot_written
    }

    /// Copy the frame texture into a mapped buffer and write it as RGB PNG.
    pub fn capture_frame(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
    ) -> Option<(wgpu::Buffer, u32, u32, u32)> {
        let (w, h) = (texture.width(), texture.height());
        let unpadded = w * 4;
        let padded = unpadded.div_ceil(256) * 256;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("screenshot"),
            size: u64::from(padded) * u64::from(h),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        Some((buffer, w, h, padded))
    }

    /// Blocks until the GPU has finished all submitted work (client
    /// shutdown: nothing is in flight when the device and surface drop).
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    pub fn finish_screenshot(&mut self, capture: (wgpu::Buffer, u32, u32, u32)) {
        let (buffer, w, h, padded) = capture;
        let Some(path) = self.pending_screenshot.take() else {
            return;
        };
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        match rx.recv() {
            Ok(Ok(())) => {}
            other => {
                log::warn!("[client910] screenshot map failed: {other:?}");
                return;
            }
        }
        let data = slice.get_mapped_range().expect("mapped screenshot range");
        let bgra = matches!(
            self.config.format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
        );
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h {
            let row = &data[(y * padded) as usize..(y * padded + w * 4) as usize];
            for px in row.chunks_exact(4) {
                if bgra {
                    rgb.extend_from_slice(&[px[2], px[1], px[0]]);
                } else {
                    rgb.extend_from_slice(&[px[0], px[1], px[2]]);
                }
            }
        }
        drop(data);
        buffer.unmap();
        match crate::png_out::write_rgb_png(&path, w, h, &rgb) {
            Ok(()) => log::info!(
                "[client910] screenshot written: {} ({w}x{h})",
                path.display()
            ),
            Err(err) => log::warn!("[client910] screenshot write failed: {err:#}"),
        }
        self.screenshot_written = true;
    }
}

/// An adapter's identity as the console prints it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdapterReport {
    /// The vendor's name, or its PCI id when it is not a known one.
    pub vendor: String,
    /// The adapter's name.
    pub name: String,
    /// The graphics API in use.
    pub version: String,
    /// The device class and PCI id.
    pub device: String,
    /// The driver's name and version text, "unknown" when it gives none.
    pub driver_version: String,
}

impl AdapterReport {
    fn from_info(info: &wgpu::AdapterInfo) -> Self {
        let vendor = match info.vendor {
            0x10de => "NVIDIA".to_owned(),
            0x1002 => "AMD".to_owned(),
            0x8086 => "Intel".to_owned(),
            0x106b => "Apple".to_owned(),
            0x13b5 => "ARM".to_owned(),
            0x5143 => "Qualcomm".to_owned(),
            other => format!("0x{other:04x}"),
        };
        let driver = format!("{} {}", info.driver, info.driver_info);
        Self {
            vendor,
            name: info.name.clone(),
            version: format!("{:?}", info.backend),
            device: format!("{:?} (0x{:04x})", info.device_type, info.device),
            driver_version: if driver.trim().is_empty() {
                "unknown".to_owned()
            } else {
                driver.trim().to_owned()
            },
        }
    }
}

/// The bytes `device` has allocated. The Metal device reports the total it
/// has allocated (`MTLDevice.currentAllocatedSize`); other backends return
/// `None`.
#[must_use]
pub fn allocated_bytes(device: &wgpu::Device) -> Option<u64> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use objc2_metal::MTLDevice;
        // SAFETY: the raw device is only read; the guard is dropped before
        // the function returns.
        unsafe {
            device
                .as_hal::<wgpu::hal::api::Metal>()
                .map(|device| device.raw_device().currentAllocatedSize() as u64)
        }
    }
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    {
        let _ = device;
        None
    }
}

/// The limits the renderers' devices request: the defaults, with the
/// per-stage sampled-texture count raised to what the modern renderer's
/// pipelines bind (its water layout binds 17 in the fragment stage; wgpu
/// releases before 23 counted only the largest bind group of a pipeline
/// layout against the limit, later ones count them all), capped at what the
/// adapter offers, the bind group count to the five the modern renderer's
/// model pipelines use (frame, material, pass inputs, lights and the material
/// arrays) and the array layer count to what its material arrays hold
/// (`models::material_arrays`).
#[must_use]
pub fn renderer_limits(adapter: &wgpu::Adapter) -> wgpu::Limits {
    let defaults = wgpu::Limits::default();
    let offered = adapter.limits();
    wgpu::Limits {
        max_sampled_textures_per_shader_stage: defaults
            .max_sampled_textures_per_shader_stage
            .max(SAMPLED_TEXTURES_PER_STAGE.min(offered.max_sampled_textures_per_shader_stage)),
        max_bind_groups: defaults
            .max_bind_groups
            .max(BIND_GROUPS.min(offered.max_bind_groups)),
        max_texture_array_layers: defaults
            .max_texture_array_layers
            .max(ARRAY_LAYERS.min(offered.max_texture_array_layers)),
        ..defaults
    }
}

/// The most bind groups a pipeline of the renderers binds.
const BIND_GROUPS: u32 = 5;

/// The array layers the modern renderer's material arrays use.
const ARRAY_LAYERS: u32 = 512;

/// The most sampled textures a stage of the renderers' pipelines binds,
/// with room to grow.
const SAMPLED_TEXTURES_PER_STAGE: u32 = 32;
