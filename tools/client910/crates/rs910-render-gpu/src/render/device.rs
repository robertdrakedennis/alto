//! Device-facing state: the renderer over the lent device
//! ([`Renderer::new`]), the surface-sized depth buffer and game canvas
//! target, resize and toolkit changes, the MSAA/bloom scene effects
//! and the capability answers (bloom and anti-aliasing support, renderer
//! plan A3's profile).

use super::*;
use std::collections::HashMap;

impl Renderer {
    /// The hardware toolkit over `gpu`, the device layer the shell created
    /// for its window and keeps (`Device::new` with
    /// [`Renderer::device_options`]): the depth buffer, the capability
    /// answers and the faithful floor/model/particle/billboard/post passes,
    /// in the order the former `Renderer::new` built them after configuring
    /// the surface.
    pub fn new(gpu: &crate::gpu_device::Device) -> Self {
        let (width, height) = (gpu.config.width, gpu.config.height);
        let depth_view = Self::create_depth(&gpu.device, width, height);
        let (msaa_samples, hdr_samples) = gpu.scene_sample_counts(DEPTH_FORMAT);
        let pipelines = crate::pipelines::PipelineCache::default();
        let floor = crate::floor_render::FloorPipeline::with_cache(
            &gpu.device,
            gpu.config.format,
            DEPTH_FORMAT,
            pipelines.clone(),
        );
        let passes = crate::floorpass::FloorPasses::cached(
            &pipelines,
            &gpu.device,
            &floor,
            gpu.config.format,
            1,
        );
        let particles = crate::particle_render::ParticlePass::new(
            &gpu.device,
            crate::particle_render::ParticlePipelines::cached(
                &pipelines,
                &gpu.device,
                &floor,
                gpu.config.format,
                1,
            ),
        );
        let billboards = crate::billboard_render::BillboardPass::new(
            &gpu.device,
            crate::billboard_render::BillboardPipelines::cached(
                &pipelines,
                &gpu.device,
                &floor,
                gpu.config.format,
                1,
            ),
        );

        let frame_profile = crate::frame_profile::FrameProfile::new(&gpu.device);
        let post = crate::postprocess::PostProcessor::new(&gpu.device);
        Self {
            msaa_samples,
            scene_target: None,
            scene_samples: 1,
            hdr_samples,
            post,
            bloom_enabled: false,
            console: None,
            ui: None,
            ui_spare: None,
            threaded_composition: false,
            composition_pending: false,
            scene_blackout: false,
            minimap_textures: std::collections::HashMap::new(),
            next_external_id: 1,
            floor,
            pipelines,
            passes,
            particles,
            billboards,
            floor_textures: crate::floor_render::FloorTextureCache::default(),
            frame_profile,
            pass_timer: None,
            scene_opaque_bundle: Vec::new(),
            scene_transparent_bundle: Vec::new(),
            underwater_models: Vec::new(),
            underwater_order: None,
            depth_view,
            size: (width, height),
            ui_scale: 1.0,
            game_canvas: None,
            canvas_target: None,
            env_passes: crate::skybox_render::EnvPasses::default(),
            retained_frame: None,
            overlay: None,
            layers: HashMap::new(),
            layer_paint: None,
        }
    }

    /// One redraw's cache work for the textures the toolkit keeps: age them,
    /// and when memory is short drop the idle ones
    /// (`rs910_core::cache_schedule`). Returns how many textures went.
    pub fn clean_caches(&mut self, frame: rs910_core::cache_schedule::Frame) -> usize {
        if frame.clean {
            self.floor_textures
                .clean(rs910_core::cache_schedule::MODEL_AGE);
        }
        if frame.clear_soft {
            return self.floor_textures.clear_soft();
        }
        0
    }

    /// The device layer's profiling switches, from this renderer's
    /// diagnostics (`CLIENT910_PROFILE`, `CLIENT910_PROFILE_PRESENT`).
    pub fn device_options() -> crate::gpu_device::DeviceOptions {
        crate::gpu_device::DeviceOptions {
            profile: crate::frame_profile::FrameProfile::enabled(),
            timestamps: rs910_core::profile::COMPILED,
            present_immediate: crate::render_debug_flags::flags().profile_present_immediate,
        }
    }

    pub(super) fn create_depth(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> wgpu::TextureView {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        texture.create_view(&wgpu::TextureViewDescriptor::default())
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn resize(&mut self, gpu: &mut crate::gpu_device::Device, width: u32, height: u32) {
        if width == 0 || height == 0 || (width == gpu.config.width && height == gpu.config.height) {
            return;
        }
        self.size = (width, height);
        gpu.config.width = width;
        gpu.config.height = height;
        gpu.configure_surface();
        self.depth_view = Self::create_depth(&gpu.device, width, height);
        self.game_canvas = None;
        self.canvas_target = None;
    }

    /// Physical pixels per game canvas pixel (`window.scale_factor()`); the
    /// canvas the UI lays out in is `size / scale`, the client's user-space
    /// units. Render targets stay physical.
    pub fn set_ui_scale(&mut self, scale: f64) {
        self.ui_scale = (scale as f32).max(1.0);
    }

    /// Game canvas size in use (`size / ui_scale`, rounded like the app).
    pub fn canvas_size(&self) -> (u32, u32) {
        if let Some(canvas) = self.game_canvas {
            return (canvas.size[0] as u32, canvas.size[1] as u32);
        }
        let w = (self.size.0 as f32 / self.ui_scale).round().max(1.0) as u32;
        let h = (self.size.1 as f32 / self.ui_scale).round().max(1.0) as u32;
        (w, h)
    }

    /// Whether the installed canvas and targets already match the current surface.
    #[must_use]
    pub fn game_canvas_matches(
        &self,
        gpu: &crate::gpu_device::Device,
        canvas: crate::game_canvas::Canvas,
    ) -> bool {
        const SURFACE_ORIGIN: i32 = 0;
        const SINGLE_SAMPLE_COUNT: u32 = 1;
        let [x, y, width, height] = canvas.physical_rect(f64::from(self.ui_scale));
        let size = (width as u32, height as u32);
        let offscreen = x != SURFACE_ORIGIN
            || y != SURFACE_ORIGIN
            || width != gpu.config.width as i32
            || height != gpu.config.height as i32
            || self.scene_samples > SINGLE_SAMPLE_COUNT;
        self.game_canvas == Some(canvas)
            && self.size == size
            && self.canvas_target.is_some() == offscreen
    }

    /// The game canvas is a child of the outer window. Render it at its own
    /// physical size, then compose it without stretching into the outer surface.
    pub fn set_game_canvas(
        &mut self,
        gpu: &crate::gpu_device::Device,
        canvas: crate::game_canvas::Canvas,
    ) -> anyhow::Result<()> {
        if self.game_canvas_matches(gpu, canvas) {
            return Ok(());
        }
        let rect = canvas.physical_rect(f64::from(self.ui_scale));
        let size = (rect[2] as u32, rect[3] as u32);
        let offscreen = rect != [0, 0, gpu.config.width as i32, gpu.config.height as i32]
            || self.scene_samples > 1;
        self.size = size;
        self.depth_view = Self::create_depth(&gpu.device, size.0, size.1);
        self.canvas_target = if !offscreen {
            None
        } else {
            Some(crate::window_canvas::CanvasTarget::with_pipeline(
                &gpu.device,
                &gpu.queue,
                gpu.config.format,
                crate::text_render_gpu::Pipeline::cached(
                    &self.pipelines,
                    &gpu.device,
                    gpu.config.format,
                ),
                [gpu.config.width, gpu.config.height],
                rect,
            )?)
        };
        self.game_canvas = Some(canvas);
        Ok(())
    }

    pub(super) fn scene_format(&self, gpu: &crate::gpu_device::Device) -> wgpu::TextureFormat {
        if self.bloom_enabled {
            wgpu::TextureFormat::Rgba16Float
        } else {
            gpu.config.format
        }
    }

    pub fn supports_scene_samples(&self, count: u32) -> bool {
        self.msaa_samples.contains(&count)
    }

    /// Whether bloom is supported on this device
    /// (`ActiveToolkit::supports_bloom` answers for toolkit 0).
    pub fn supports_bloom(&self) -> bool {
        self.hdr_samples.contains(&1)
    }

    /// Whether anti-aliasing is supported on this device
    /// (`ActiveToolkit::supports_antialiasing` answers for toolkit 0).
    pub fn supports_antialiasing(&self) -> bool {
        self.supports_scene_samples(2) && self.supports_scene_samples(4)
    }

    /// This device's capability answers as data (renderer plan A3): the
    /// scene and HDR sample sets the three answers above read and the GL
    /// texture formats the input telemetry reports
    /// (`client_watch_gpu::gl_compressed_texture_formats`).
    pub fn capability_profile(
        &self,
        gpu: &crate::gpu_device::Device,
    ) -> rs910_toolkit::capability::Profile {
        rs910_toolkit::capability::Profile {
            scene_samples: self.msaa_samples.iter().copied().collect(),
            hdr_samples: self.hdr_samples.iter().copied().collect(),
            compressed_texture_formats:
                rs910_gpu_device::client_watch_gpu::gl_compressed_texture_formats(
                    gpu.adapter_features(),
                ),
        }
    }

    /// Whether the authoritative scene targets already have these settings.
    #[must_use]
    pub fn scene_effects_match(&self, count: u32, bloom: bool) -> bool {
        self.scene_samples == count && self.bloom_enabled == bloom
    }

    pub fn set_scene_effects(
        &mut self,
        gpu: &crate::gpu_device::Device,
        count: u32,
        bloom: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.supports_scene_samples(count),
            "device does not support {count} scene samples"
        );
        anyhow::ensure!(
            !bloom || self.hdr_samples.contains(&count),
            "device does not support bloom with {count} scene samples"
        );
        if self.scene_effects_match(count, bloom) {
            return Ok(());
        }
        // The bloom effect joins or leaves the post-process chain.
        if bloom {
            self.post.chain.add(crate::postprocess::Effect::Bloom);
        } else {
            self.post.chain.remove(crate::postprocess::Effect::Bloom);
        }
        let format = if bloom {
            wgpu::TextureFormat::Rgba16Float
        } else {
            gpu.config.format
        };
        self.floor
            .set_sample_count(&gpu.device, format, DEPTH_FORMAT, count);
        self.passes = crate::floorpass::FloorPasses::cached(
            &self.pipelines,
            &gpu.device,
            &self.floor,
            format,
            count,
        );
        self.particles = crate::particle_render::ParticlePass::new(
            &gpu.device,
            crate::particle_render::ParticlePipelines::cached(
                &self.pipelines,
                &gpu.device,
                &self.floor,
                format,
                count,
            ),
        );
        self.billboards = crate::billboard_render::BillboardPass::new(
            &gpu.device,
            crate::billboard_render::BillboardPipelines::cached(
                &self.pipelines,
                &gpu.device,
                &self.floor,
                format,
                count,
            ),
        );
        self.scene_opaque_bundle.clear();
        self.scene_transparent_bundle.clear();
        self.scene_target = None;
        self.scene_samples = count;
        self.bloom_enabled = bloom;
        if crate::toolkit_debug_flags::flags().settings_trace {
            log::info!("[graphics] scene samples={count} bloom={bloom}");
        }
        Ok(())
    }

    /// After a toolkit change: release the surface-sized and scene targets
    /// and rebuild them with the new sample count/bloom state. The toolkit
    /// change itself makes a new device first (`ActiveToolkit::recreate_device`)
    /// and the caller's model-cache reset and scene rebuild upload the world
    /// to it.
    pub fn recreate_toolkit_targets(
        &mut self,
        gpu: &crate::gpu_device::Device,
        samples: u32,
        bloom: bool,
    ) -> anyhow::Result<()> {
        gpu.configure_surface();
        self.depth_view = Self::create_depth(&gpu.device, self.size.0, self.size.1);
        self.game_canvas = None;
        self.canvas_target = None;
        // Force the scene passes/targets to be rebuilt even when the sample
        // count and bloom state are unchanged.
        self.scene_samples = 0;
        self.set_scene_effects(gpu, samples, bloom)
    }
}
