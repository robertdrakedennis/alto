//! Frame orchestration: acquire ([`Renderer::begin_frame`]), the UI below
//! the scene, the scene (the faithful passes in `scene_pass`, or another
//! backend's through [`Renderer::frame_composite`]), the UI over it, the
//! console, the retained copy, the canvas scale-up, a requested screenshot,
//! submit and present.

use super::*;

/// What [`Renderer::frame_composite`] hands the scene backend: the device,
/// this frame's encoder and colour target (`size` pixels of `format`, the
/// UI already drawn below the scene) and the scene's `viewport` in target
/// pixels (`rect` `[x, y, w, h]`, `clip` `[l, t, r, b]`).
pub struct SceneTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub view: &'a wgpu::TextureView,
    pub format: wgpu::TextureFormat,
    pub size: [u32; 2],
    pub viewport: crate::frame_plan::Scene,
}

/// Frames skipped in a row because the surface gave no texture (occluded
/// window, acquire timeout): logged on the first skip, every
/// [`SKIP_LOG_EVERY`]th after it and when frames resume, so an online run's
/// log says why the screen did not update (every renderer composes through
/// [`Renderer::begin_frame`], the scene renderers included).
static SKIPPED_FRAMES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const SKIP_LOG_EVERY: u64 = 300;

/// A frame is skipped: the surface is `reason` (`"occluded"` or `"timed out"`).
pub(super) fn note_skipped_frame(reason: &str) {
    let n = SKIPPED_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    if n == 1 || n.is_multiple_of(SKIP_LOG_EVERY) {
        log::info!("[client910] frame skipped: the surface is {reason} (skipped {n} in a row)");
    }
}

/// A frame was drawn: reports an earlier run of skipped ones.
pub(super) fn note_frame_acquired() {
    let n = SKIPPED_FRAMES.swap(0, std::sync::atomic::Ordering::Relaxed);
    if n > 0 {
        log::info!("[client910] frames resume after {n} skipped (surface occluded or timed out)");
    }
}

/// The `Image::External` id of [`Renderer`]'s retained frame in the overlay
/// painter.
pub const RETAINED_FRAME_ID: u64 = u64::MAX;

/// Copy `source` (the frame's canvas or surface texture) into `slot`.
pub(super) fn retain_frame(
    device: &wgpu::Device,
    encoder: &mut wgpu::CommandEncoder,
    slot: &mut Option<wgpu::Texture>,
    source: &wgpu::Texture,
) {
    let size = source.size();
    if slot
        .as_ref()
        .is_none_or(|t| t.size() != size || t.format() != source.format())
    {
        *slot = Some(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("retained frame"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: source.format(),
            usage: wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        }));
    }
    encoder.copy_texture_to_texture(
        source.as_image_copy(),
        slot.as_ref().unwrap().as_image_copy(),
        size,
    );
}

/// Copy a completed canvas without sampling, blending or repainting it.
pub(super) fn copy_retained_canvas(
    encoder: &mut wgpu::CommandEncoder,
    retained: &wgpu::Texture,
    target: &wgpu::Texture,
) {
    encoder.copy_texture_to_texture(
        retained.as_image_copy(),
        target.as_image_copy(),
        retained.size(),
    );
}

/// One frame of the faithful composition (renderer plan M1): the surface
/// texture acquired for it and the colour target the UI, the console and
/// the scene draw into (the game canvas target when the canvas is bounded
/// inside the window, otherwise the surface). [`Renderer::begin_frame`]
/// acquires it; a scene backend other than the faithful one (the NXT
/// renderer) encodes into [`Frame::view`] between
/// [`Renderer::encode_ui_under`] and [`Renderer::encode_ui_over`], then
/// [`Renderer::end_frame`] submits and presents it.
pub struct Frame {
    pub(super) output: wgpu::SurfaceTexture,
    pub(super) surface_view: wgpu::TextureView,
    pub(super) canvas_view: Option<wgpu::TextureView>,
}

impl Frame {
    /// The colour target of the frame: the game canvas target, or the
    /// surface. Its size is [`Renderer::size`] and its format the surface
    /// configuration's.
    pub fn view(&self) -> &wgpu::TextureView {
        self.canvas_view.as_ref().unwrap_or(&self.surface_view)
    }
}

impl Renderer {
    /// Require a completed frame from the newly selected scene backend before
    /// presenting its canvas. Backend selection does not always recreate a device.
    pub fn invalidate_retained_frame(&mut self) {
        self.retained_frame = None;
    }

    /// Present the retained canvas byte-for-byte, with the same bounded-canvas
    /// placement and screenshot path. False asks the caller for a full redraw
    /// when the canvas has changed or the surface does not support copies.
    pub fn present_retained(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
    ) -> anyhow::Result<bool> {
        let Some(retained) = self.retained_frame.as_ref() else {
            return Ok(false);
        };
        if retained.width() != self.size.0
            || retained.height() != self.size.1
            || retained.format() != gpu.config.format
            || (self.canvas_target.is_none()
                && !gpu.config.usage.contains(wgpu::TextureUsages::COPY_DST))
        {
            return Ok(false);
        }
        let Some(frame) = rs910_core::profile::scope!("frame acquire", self.begin_frame(gpu)?)
        else {
            return Ok(true);
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("retained present"),
            });
        let retained = self.retained_frame.as_ref().expect("retained frame");
        let target = self
            .canvas_target
            .as_ref()
            .map_or(&frame.output.texture, |canvas| &canvas.texture);
        copy_retained_canvas(&mut encoder, retained, target);
        if let Some(canvas) = &self.canvas_target {
            canvas.encode(&mut encoder, &frame.surface_view);
        }
        rs910_core::profile::scope!("frame submit", Self::end_frame(gpu, encoder, frame));
        Ok(true)
    }

    /// Draw one frame: the UI's scene rectangle gets the sky, then the
    /// faithful scene (`scene_opaque` entities, every
    /// level's floor with its lights and hard shadows, then
    /// `scene_transparent` entities, each followed by its particles), the
    /// post-process chain, the UI, the console and the canvas scale-up.
    pub fn frame_with_levels(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        camera: &OrbitCamera,
        env: &crate::env::EnvFrame,
        floors: &[crate::floorpass::FloorLevelDraw<'_>],
        scene_opaque: &[&crate::floor_render::FloorMesh],
        scene_transparent: &[&crate::floor_render::FloorMesh],
    ) -> anyhow::Result<()> {
        let post_bounds = self.postprocess_bounds();
        self.ensure_scene_target(gpu, post_bounds);
        let profile_start = self.frame_profile.as_mut().map(|profile| {
            profile.begin();
            std::time::Instant::now()
        });
        let elapsed = || profile_start.map_or(0.0, |start| start.elapsed().as_secs_f64() * 1000.0);
        let viewport = self.scene_size();
        self.floor
            .update_camera(gpu, &camera.scene_camera(viewport), env);
        let scene_format = self.scene_format(gpu);
        let bundle_models = !crate::render_debug_flags::flags().profile_direct_draws;
        let lists = rs910_core::profile::scope!(
            "frame scene lists",
            self.scene_lists(
                gpu,
                floors,
                scene_opaque,
                scene_transparent,
                scene_format,
                bundle_models,
            )
        );
        if self.sky_drawn() {
            let viewport = self.scene_size();
            self.env_passes.upload_flat(&gpu.device, gpu, viewport);
        }
        let prepared = elapsed();
        let Some(frame) = rs910_core::profile::scope!("frame acquire", self.begin_frame(gpu)?)
        else {
            return Ok(());
        };
        let acquired = elapsed();
        let encode = rs910_core::profile::Scope::enter("frame encode");
        self.begin_pass_timer(gpu);
        let view = frame.view();
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("multi-level frame"),
            });
        self.pass_mark(&mut encoder, "gpu ui under");
        self.encode_ui_under(gpu, &mut encoder, &frame);
        self.pass_mark(&mut encoder, "gpu sky");
        let sky_drawn = self.encode_sky_pass(&mut encoder, view, env);
        self.pass_mark(&mut encoder, "gpu scene");
        self.encode_scene_pass(&mut encoder, view, env, sky_drawn, &lists);
        self.pass_mark(&mut encoder, "gpu post");
        self.encode_post_pass(gpu, &mut encoder, view, scene_format, post_bounds);
        self.pass_mark(&mut encoder, "gpu ui over");
        self.encode_ui_over(gpu, &mut encoder, &frame);
        self.pass_mark(&mut encoder, "");
        let capture = if gpu.pending_screenshot.is_some() {
            gpu.capture_frame(&mut encoder, &frame.output.texture)
        } else {
            None
        };
        if let Some(profile) = &self.frame_profile {
            profile.resolve(&mut encoder);
        }
        if let Some(timer) = self.pass_timer() {
            timer.resolve(&mut encoder);
        }
        let commands = encoder.finish();
        encode.end();
        let encoded = elapsed();
        rs910_core::profile::scope!("frame submit", gpu.submit(std::iter::once(commands)));
        if let Some(timer) = self.pass_timer() {
            timer.submitted();
        }
        let submitted = elapsed();
        if let Some(capture) = capture {
            gpu.finish_screenshot(capture);
        }
        rs910_core::profile::scope!("frame present", gpu.queue.present(frame.output));
        if let Some(profile) = &mut self.frame_profile {
            profile.finish(
                &gpu.device,
                &gpu.queue,
                [
                    prepared,
                    acquired - prepared,
                    encoded - acquired,
                    submitted - encoded,
                    elapsed() - submitted,
                ],
                scene_opaque
                    .iter()
                    .chain(scene_transparent.iter())
                    .map(|m| m.batch_count())
                    .sum(),
                self.size,
            );
        }
        Ok(())
    }

    /// Acquire this frame's surface texture ([`Frame`]); `None` skips the
    /// frame (a lost or outdated surface is reconfigured and the depth
    /// buffer recreated, a timeout drops the frame), as the faithful frame
    /// always did.
    pub fn begin_frame(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
    ) -> anyhow::Result<Option<Frame>> {
        let output = match gpu.acquire() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                gpu.configure_surface();
                self.depth_view = Self::create_depth(&gpu.device, self.size.0, self.size.1);
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                note_skipped_frame("timed out");
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                note_skipped_frame("occluded");
                return Ok(None);
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow::anyhow!("acquire surface texture: validation error"));
            }
        };
        note_frame_acquired();
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let canvas_view = self.canvas_target.as_ref().map(|c| {
            c.texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        Ok(Some(Frame {
            output,
            surface_view,
            canvas_view,
        }))
    }

    /// The retained UI below the scene (the paint ops before the scene
    /// component and the interface models among them) into `frame`.
    pub fn encode_ui_under(
        &self,
        gpu: &crate::gpu_device::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
    ) {
        if let Some(ui) = &self.ui {
            ui.encode(&gpu.device, encoder, frame.view(), &self.depth_view, true);
        }
    }

    /// Where this frame's scene draws, in [`Frame::view`] pixels: the UI's
    /// scene component (`rect` the viewport, `clip` its scissor as `[l, t,
    /// r, b]`), or the whole target without a retained UI; `None` when no
    /// scene draws (a UI without a scene component, or the
    /// scene blackout, see `set_scene_blackout`).
    pub fn scene_viewport(&self) -> Option<crate::frame_plan::Scene> {
        if self.scene_blackout {
            return None;
        }
        match self.latest_ui() {
            None => {
                let (w, h) = (self.size.0 as i32, self.size.1 as i32);
                Some(crate::frame_plan::Scene {
                    rect: [0, 0, w, h],
                    clip: [0, 0, w, h],
                })
            }
            Some(ui) => ui.scene.as_ref().map(|s| crate::frame_plan::Scene {
                rect: s.rect,
                clip: s.clip,
            }),
        }
    }

    /// The retained UI over the scene, the developer console, the retained
    /// copy of the frame and the canvas scale-up into
    /// `frame`.
    pub fn encode_ui_over(
        &mut self,
        gpu: &crate::gpu_device::Device,
        encoder: &mut wgpu::CommandEncoder,
        frame: &Frame,
    ) {
        let view = frame.view();
        if let Some(ui) = &self.ui {
            ui.encode(&gpu.device, encoder, view, &self.depth_view, false);
        }
        if let Some(console) = &self.console {
            console.encode(encoder, view);
        }
        retain_frame(
            &gpu.device,
            encoder,
            &mut self.retained_frame,
            self.canvas_target
                .as_ref()
                .map_or(&frame.output.texture, |c| &c.texture),
        );
        if let Some(canvas) = &self.canvas_target {
            canvas.encode(encoder, &frame.surface_view);
        }
    }

    /// Read a requested screenshot back, submit `encoder` and present
    /// `frame`.
    pub fn end_frame(
        gpu: &mut crate::gpu_device::Device,
        encoder: wgpu::CommandEncoder,
        frame: Frame,
    ) {
        Self::end_frame_with_commands(gpu, Vec::new(), encoder, frame);
    }

    /// Submit a backend's ordered units together with the final canvas encoder.
    pub(super) fn end_frame_with_commands(
        gpu: &mut crate::gpu_device::Device,
        commands: Vec<wgpu::CommandBuffer>,
        mut encoder: wgpu::CommandEncoder,
        frame: Frame,
    ) {
        let capture = if gpu.pending_screenshot.is_some() {
            gpu.capture_frame(&mut encoder, &frame.output.texture)
        } else {
            None
        };
        gpu.submit(
            commands
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        if let Some(capture) = capture {
            gpu.finish_screenshot(capture);
        }
        gpu.queue.present(frame.output);
    }

    /// A frame whose scene another backend draws (renderer plan M1, the
    /// NXT renderer): the faithful composition of [`Renderer::frame_with_levels`]
    /// (acquire, the UI below the scene, the scene, the UI over it, the
    /// console, the retained copy, the canvas scale-up, a requested
    /// screenshot, present) with `scene` encoding the scene into the frame's
    /// target at [`Renderer::scene_viewport`] instead of the faithful passes
    /// and the post-process chain. `scene` is not called when no scene
    /// draws.
    pub fn frame_composite(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        scene: impl FnOnce(SceneTarget<'_>) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        self.frame_composite_with_commands(gpu, |target| {
            scene(target)?;
            Ok(Vec::new())
        })
    }

    /// The composition with backend command buffers submitted before its final
    /// canvas encoder, in the same submission as the scene's post chain.
    pub fn frame_composite_with_commands(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        scene: impl FnOnce(SceneTarget<'_>) -> anyhow::Result<Vec<wgpu::CommandBuffer>>,
    ) -> anyhow::Result<()> {
        let Some(frame) = rs910_core::profile::scope!("frame acquire", self.begin_frame(gpu)?)
        else {
            return Ok(());
        };
        self.begin_pass_timer(gpu);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("composite frame"),
            });
        self.pass_mark(&mut encoder, "gpu ui under");
        self.encode_ui_under(gpu, &mut encoder, &frame);
        self.pass_mark(&mut encoder, "gpu scene (backend)");
        let drawn = match self.scene_viewport() {
            Some(viewport) => scene(SceneTarget {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut encoder,
                view: frame.view(),
                format: gpu.config.format,
                size: [self.size.0, self.size.1],
                viewport,
            }),
            None => Ok(Vec::new()),
        };
        self.pass_mark(&mut encoder, "gpu ui over");
        self.encode_ui_over(gpu, &mut encoder, &frame);
        self.pass_mark(&mut encoder, "");
        if let Some(timer) = self.pass_timer() {
            timer.resolve(&mut encoder);
        }
        let (commands, result) = match drawn {
            Ok(commands) => (commands, Ok(())),
            Err(error) => (Vec::new(), Err(error)),
        };
        rs910_core::profile::scope!(
            "frame submit",
            Self::end_frame_with_commands(gpu, commands, encoder, frame)
        );
        if let Some(timer) = self.pass_timer() {
            timer.submitted();
        }
        result
    }

    /// The engine profiler's pass timer for this frame (lane E-A4): created
    /// on the first frame the profiler records (adapters with
    /// `TIMESTAMP_QUERY`), idle while it does not.
    fn begin_pass_timer(&mut self, gpu: &crate::gpu_device::Device) {
        if !rs910_core::profile::COMPILED {
            return;
        }
        let frame =
            rs910_core::profile::current_frame().filter(|_| rs910_core::profile::gpu_timing());
        if frame.is_some() && self.pass_timer.is_none() {
            self.pass_timer = crate::frame_profile::PassTimer::new(&gpu.device, &gpu.queue);
        }
        if let Some(timer) = self.pass_timer() {
            timer.begin(&gpu.device, frame);
        }
    }

    /// The pass timer; `None` when the profiler is compiled out, so none of
    /// its calls remain in a default build.
    fn pass_timer(&mut self) -> Option<&mut crate::frame_profile::PassTimer> {
        if rs910_core::profile::COMPILED {
            self.pass_timer.as_mut()
        } else {
            None
        }
    }

    /// A GPU pass boundary of the profiled frame: `name` starts here.
    fn pass_mark(&mut self, encoder: &mut wgpu::CommandEncoder, name: &'static str) {
        if let Some(timer) = self.pass_timer() {
            timer.mark(encoder, name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Presenting copies the complete canvas, including transparent texels,
    /// rather than sampling or blending the retained frame again.
    #[test]
    #[ignore = "requires a GPU adapter"]
    fn retained_canvas_copies_preserve_every_byte_and_take_the_latest_frame() -> anyhow::Result<()>
    {
        const WIDTH: u32 = 64;
        const HEIGHT: u32 = 8;
        const CHANNELS: u32 = 4;
        const ROW_BYTES: u32 = WIDTH * CHANNELS;
        const FRAME_SEEDS: [u8; 2] = [17, 93];
        const REPEAT_PRESENTS: usize = 3;
        const CANVAS_ORIGIN: [i32; 2] = [0; 2];
        const INSET_CANVAS_SIZE: [i32; 2] = [32, HEIGHT as i32];
        const INSET_CANVAS_OFFSET: [i32; 2] = [4, 0];
        const SAMPLE_CHOICES: [u32; 2] = [2, 4];
        const UI_SCALE: f64 = 2.0;
        const SCALED_SIZE: (u32, u32) = (128, 16);
        let mut gpu = pollster::block_on(crate::gpu_device::Device::headless(
            (WIDTH, HEIGHT),
            Default::default(),
            wgpu::Features::empty(),
        ))?;
        let mut renderer = Renderer::new(&gpu);
        for format in [
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureFormat::Bgra8Unorm,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        ] {
            let texture = |label| {
                gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: WIDTH,
                        height: HEIGHT,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                })
            };
            let source = texture("full frame");
            let output = texture("presented frame");
            for seed in FRAME_SEEDS {
                let pixels: Vec<u8> = (0..ROW_BYTES * HEIGHT)
                    .map(|i| (i as u8).wrapping_add(seed))
                    .collect();
                gpu.queue.write_texture(
                    source.as_image_copy(),
                    &pixels,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(ROW_BYTES),
                        rows_per_image: Some(HEIGHT),
                    },
                    source.size(),
                );
                let mut encoder = gpu.device.create_command_encoder(&Default::default());
                retain_frame(
                    &gpu.device,
                    &mut encoder,
                    &mut renderer.retained_frame,
                    &source,
                );
                gpu.submit([encoder.finish()]);
                for _ in 0..REPEAT_PRESENTS {
                    let retained = renderer.retained_frame.as_ref().unwrap();
                    let readback = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("retained present readback"),
                        size: u64::from(ROW_BYTES * HEIGHT),
                        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                        mapped_at_creation: false,
                    });
                    let mut encoder = gpu.device.create_command_encoder(&Default::default());
                    copy_retained_canvas(&mut encoder, retained, &output);
                    encoder.copy_texture_to_buffer(
                        output.as_image_copy(),
                        wgpu::TexelCopyBufferInfo {
                            buffer: &readback,
                            layout: wgpu::TexelCopyBufferLayout {
                                offset: 0,
                                bytes_per_row: Some(ROW_BYTES),
                                rows_per_image: Some(HEIGHT),
                            },
                        },
                        output.size(),
                    );
                    gpu.submit([encoder.finish()]);
                    let (send, receive) = std::sync::mpsc::channel();
                    readback
                        .slice(..)
                        .map_async(wgpu::MapMode::Read, move |result| {
                            send.send(result).unwrap();
                        });
                    let _ = gpu.device.poll(wgpu::PollType::wait_indefinitely());
                    receive.recv()??;
                    assert_eq!(
                        &*readback.slice(..).get_mapped_range().expect("mapped range"),
                        pixels.as_slice()
                    );
                }
                if format == gpu.config.format {
                    assert!(renderer.present_retained(&mut gpu)?);
                }
                renderer.invalidate_retained_frame();
                assert!(renderer.retained_frame.is_none());
                assert!(!renderer.present_retained(&mut gpu)?);
            }
        }
        let canvas = crate::game_canvas::Canvas {
            size: [WIDTH as i32, HEIGHT as i32],
            offset: CANVAS_ORIGIN,
        };
        let inset = crate::game_canvas::Canvas {
            size: INSET_CANVAS_SIZE,
            offset: INSET_CANVAS_OFFSET,
        };
        assert!(!renderer.game_canvas_matches(&gpu, canvas));
        renderer.set_game_canvas(&gpu, canvas)?;
        let full_depth = renderer.depth_view.clone();
        assert!(renderer.canvas_target.is_none());
        assert!(renderer.game_canvas_matches(&gpu, canvas));
        renderer.set_game_canvas(&gpu, canvas)?;
        assert_eq!(
            renderer.depth_view, full_depth,
            "equal canvas preserves depth"
        );

        assert!(!renderer.game_canvas_matches(&gpu, inset));
        renderer.set_game_canvas(&gpu, inset)?;
        let inset_depth = renderer.depth_view.clone();
        let inset_texture = renderer.canvas_target.as_ref().unwrap().texture.clone();
        assert_ne!(inset_depth, full_depth, "changed canvas rebuilds depth");
        assert!(renderer.game_canvas_matches(&gpu, inset));
        renderer.set_game_canvas(&gpu, inset)?;
        assert_eq!(renderer.depth_view, inset_depth);
        assert_eq!(
            renderer.canvas_target.as_ref().unwrap().texture,
            inset_texture
        );

        renderer.set_game_canvas(&gpu, canvas)?;
        assert!(renderer.canvas_target.is_none());
        let samples = SAMPLE_CHOICES
            .into_iter()
            .find(|&count| renderer.supports_scene_samples(count))
            .expect("a multisample target");
        renderer.set_scene_effects(&gpu, samples, false)?;
        // Logical value and dimensions match, but MSAA now requires offscreen ownership.
        assert!(!renderer.game_canvas_matches(&gpu, canvas));
        let before_msaa = renderer.depth_view.clone();
        renderer.set_game_canvas(&gpu, canvas)?;
        assert!(renderer.game_canvas_matches(&gpu, canvas));
        assert_ne!(renderer.depth_view, before_msaa);
        assert!(renderer.canvas_target.is_some());

        let before_scale = renderer.depth_view.clone();
        let texture_before_scale = renderer.canvas_target.as_ref().unwrap().texture.clone();
        renderer.set_ui_scale(UI_SCALE);
        assert!(!renderer.game_canvas_matches(&gpu, canvas));
        renderer.set_game_canvas(&gpu, canvas)?;
        assert_eq!(renderer.size(), SCALED_SIZE);
        assert_ne!(renderer.depth_view, before_scale);
        assert_ne!(
            renderer.canvas_target.as_ref().unwrap().texture,
            texture_before_scale
        );
        assert!(renderer.game_canvas_matches(&gpu, canvas));
        Ok(())
    }
}
