//! The retained UI (`RetainedUi`: the painter and interface models of the
//! frame's `ui_output::Output`), its offscreen framebuffer sprites, the
//! message-box frame over the retained game frame, and the developer
//! console's GPU renderer.

use super::*;

pub(super) struct RetainedUi {
    pub(super) models: crate::ui_model_gpu::Models,
    pub(super) paint: crate::ui_paint_gpu::Renderer,
    pub(super) scene: Option<crate::frame_plan::Scene>,
    pub(super) split: usize,
    /// Physical `[x, y, w, h]` bounds of this frame's post-process capture
    /// (`ui_backend::PostRegion`).
    pub(super) postprocess: Option<[u32; 4]>,
    pub(super) layer_targets: std::collections::HashMap<u64, wgpu::Texture>,
    pub(super) layer_painters: std::collections::HashMap<u64, crate::ui_paint_gpu::Renderer>,
    pub(super) layer_frames: Vec<super::composition::LayerFrame>,
}

impl RetainedUi {
    /// An empty retained UI whose painter and interface models draw with
    /// `pipelines`.
    pub(super) fn new(
        gpu: &crate::gpu_device::Device,
        pipelines: &crate::pipelines::PipelineCache,
        minimaps: &std::collections::HashMap<u64, wgpu::Texture>,
    ) -> Self {
        let mut paint = crate::ui_paint_gpu::Renderer::with_pipeline(
            crate::text_render_gpu::Pipeline::cached(pipelines, &gpu.device, gpu.config.format),
        );
        for (&id, texture) in minimaps {
            paint.register_external(&gpu.device, id, texture.create_view(&Default::default()));
        }
        Self {
            models: crate::ui_model_gpu::Models::with_pipelines(
                &gpu.device,
                gpu.config.format,
                pipelines.clone(),
            ),
            paint,
            scene: None,
            split: 0,
            postprocess: None,
            layer_targets: std::collections::HashMap::new(),
            layer_painters: std::collections::HashMap::new(),
            layer_frames: Vec::new(),
        }
    }

    pub(super) fn encode(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        before: bool,
    ) {
        self.models.encode(
            device,
            &self.paint,
            self.split,
            encoder,
            crate::ui_model_gpu::ModelTargets { view, depth },
            before,
        );
    }
}

impl Renderer {
    pub fn prepare_ui(
        &mut self,
        gpu: &crate::gpu_device::Device,
        output: crate::ui_output::Output<crate::interface_model::Draw>,
    ) -> anyhow::Result<()> {
        self.composition_pending = false;
        let (cw, ch) = self.canvas_size();
        // Bounds-check the game-canvas rect as well, so a layout bug that
        // only shows on HiDPI (canvas smaller than the surface) fails loudly.
        if let Some(orig) = &output.scene {
            let [x, y, w, h] = orig.rect;
            anyhow::ensure!(
                x >= 0 && y >= 0 && w > 0 && h > 0 && x + w <= cw as i32 && y + h <= ch as i32,
                "UI scene viewport outside canvas: {:?}",
                orig.rect
            );
        }
        let scene = output.scene.map(|s| {
            let map = |bounds| {
                crate::ui_paint::framebuffer_bounds(bounds, [cw, ch], [self.size.0, self.size.1])
            };
            let [x, y, w, h] = s.rect;
            let [l, t, r, b] = map([x, y, x + w, y + h]);
            crate::frame_plan::Scene {
                rect: [l, t, r - l, b - t],
                clip: map(s.clip),
            }
        });
        if let Some(scene) = &scene {
            let [x, y, w, h] = scene.rect;
            anyhow::ensure!(
                x >= 0
                    && y >= 0
                    && w > 0
                    && h > 0
                    && x + w <= self.size.0 as i32
                    && y + h <= self.size.1 as i32,
                "UI scene viewport outside target: {:?}",
                scene.rect
            );
        }
        let layers = if self.threaded_composition {
            let ui = self.ui.get_or_insert_with(|| {
                RetainedUi::new(gpu, &self.pipelines, &self.minimap_textures)
            });
            super::composition::prepare_layers(
                gpu,
                &self.pipelines,
                ui,
                output.layers,
                [self.size.0, self.size.1],
            )?
        } else {
            self.render_layers(gpu, output.layers)?
        };
        let ui = self
            .ui
            .get_or_insert_with(|| RetainedUi::new(gpu, &self.pipelines, &self.minimap_textures));
        for (id, view) in layers {
            ui.paint.register_external(&gpu.device, id, view);
        }
        let boundaries: Vec<_> = std::iter::once(output.scene_quad)
            .chain(output.models.iter().map(|d| d.quad))
            .collect();
        ui.paint.prepare_boundaries_target(
            &gpu.device,
            gpu,
            output.paint,
            &boundaries,
            [self.size.0, self.size.1],
        )?;
        ui.models.bloom = self.bloom_enabled;
        ui.models.set_samples(
            &gpu.device,
            gpu.config.format,
            [self.size.0, self.size.1],
            if output.models.is_empty() {
                1
            } else {
                self.scene_samples
            },
        );
        ui.models.prepare(
            &gpu.device,
            gpu,
            output.models,
            [cw, ch],
            [self.size.0, self.size.1],
        )?;
        ui.scene = scene;
        ui.split = output.scene_quad;
        ui.postprocess = output.postprocess.map(|region| {
            let [x, y, w, h] = region.rect;
            let [l, t, r, b] = crate::ui_paint::framebuffer_bounds(
                [x, y, x.saturating_add(w), y.saturating_add(h)],
                [cw, ch],
                [self.size.0, self.size.1],
            );
            [
                l.max(0) as u32,
                t.max(0) as u32,
                (r - l).max(0) as u32,
                (b - t).max(0) as u32,
            ]
        });
        Ok(())
    }

    /// The message box in a rebuild state: `plan` is painted over the
    /// retained last game frame (the front buffer copy) and presented at
    /// once. Without a retained frame the canvas is black. The developer
    /// console draws after it.
    pub fn frame_message_box(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        plan: crate::ui_paint::Plan,
    ) -> anyhow::Result<()> {
        self.frame_message_box_over(
            gpu,
            plan,
            rs910_toolkit::performance_metric::Backdrop::LastFrame,
        )
    }

    /// [`Self::frame_message_box`] over `backdrop`: the retained last frame,
    /// or a cleared canvas (a toolkit that was just made has no last frame).
    pub fn frame_message_box_over(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        plan: crate::ui_paint::Plan,
        backdrop: rs910_toolkit::performance_metric::Backdrop,
    ) -> anyhow::Result<()> {
        let canvas = plan.size;
        let target = [self.size.0, self.size.1];
        let overlay = self.overlay.get_or_insert_with(|| {
            crate::ui_paint_gpu::Renderer::with_pipeline(crate::text_render_gpu::Pipeline::cached(
                &self.pipelines,
                &gpu.device,
                gpu.config.format,
            ))
        });
        let mut painter = crate::ui_paint::Painter::new(canvas);
        let kept = match backdrop {
            rs910_toolkit::performance_metric::Backdrop::LastFrame => self.retained_frame.as_ref(),
            rs910_toolkit::performance_metric::Backdrop::Black => None,
        };
        if let Some(frame) = kept {
            overlay.register_external(
                &gpu.device,
                RETAINED_FRAME_ID,
                frame.create_view(&wgpu::TextureViewDescriptor::default()),
            );
            let [w, h] = canvas.map(|v| v as f32);
            painter.affine_image(
                crate::ui_paint::Image::External(RETAINED_FRAME_ID),
                [0.0, 0.0, w, 0.0, 0.0, h],
                -1,
                None,
            );
        }
        let mut quads = painter.finish().quads;
        quads.extend(plan.quads);
        overlay.prepare_split_target(
            &gpu.device,
            gpu,
            crate::ui_paint::Plan {
                size: canvas,
                quads,
            },
            None,
            target,
        )?;
        let output = match gpu.acquire() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                gpu.configure_surface();
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Timeout => {
                super::frame::note_skipped_frame("timed out");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Occluded => {
                super::frame::note_skipped_frame("occluded");
                return Ok(());
            }
            wgpu::CurrentSurfaceTexture::Validation => {
                return Err(anyhow::anyhow!("acquire surface texture: validation error"));
            }
        };
        super::frame::note_frame_acquired();
        let surface_view = output
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        let canvas_view = self.canvas_target.as_ref().map(|c| {
            c.texture
                .create_view(&wgpu::TextureViewDescriptor::default())
        });
        let view = canvas_view.as_ref().unwrap_or(&surface_view);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("message box frame"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("message box"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            self.overlay.as_ref().unwrap().draw(&mut pass);
        }
        if let Some(console) = &self.console {
            console.encode(&mut encoder, view);
        }
        if let Some(canvas) = &self.canvas_target {
            canvas.encode(&mut encoder, &surface_view);
        }
        let capture = if gpu.pending_screenshot.is_some() {
            gpu.capture_frame(&mut encoder, &output.texture)
        } else {
            None
        };
        gpu.submit(std::iter::once(encoder.finish()));
        if let Some(capture) = capture {
            gpu.finish_screenshot(capture);
        }
        gpu.queue.present(output);
        Ok(())
    }

    /// Draw each of this frame's offscreen layers (frame-buffer sprites,
    /// as the loading screen renderer captures them) into its own target, cleared to
    /// the fresh sprite's transparent black, and register it with `paint`.
    fn render_layers(
        &mut self,
        gpu: &crate::gpu_device::Device,
        layers: Vec<(u64, crate::ui_paint::Plan)>,
    ) -> anyhow::Result<Vec<(u64, wgpu::TextureView)>> {
        let target = [self.size.0, self.size.1];
        let mut views = Vec::with_capacity(layers.len());
        for (id, plan) in layers {
            let size = wgpu::Extent3d {
                width: target[0],
                height: target[1],
                depth_or_array_layers: 1,
            };
            let texture = self.layers.entry(id).or_insert_with(|| {
                gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("framebuffer sprite"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: gpu.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
            });
            if texture.size() != size {
                *texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("framebuffer sprite"),
                    size,
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: gpu.config.format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
            }
            let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
            let paint = self.layer_paint.get_or_insert_with(|| {
                crate::ui_paint_gpu::Renderer::with_pipeline(
                    crate::text_render_gpu::Pipeline::cached(
                        &self.pipelines,
                        &gpu.device,
                        gpu.config.format,
                    ),
                )
            });
            paint.prepare_split_target(&gpu.device, gpu, plan, None, target)?;
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("framebuffer sprite"),
                });
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("framebuffer sprite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                    timestamp_writes: None,
                });
                paint.draw(&mut pass);
            }
            gpu.submit(std::iter::once(encoder.finish()));
            views.push((
                id,
                texture.create_view(&wgpu::TextureViewDescriptor::default()),
            ));
        }
        Ok(views)
    }

    /// See `scene_blackout`; the UI pass before the scene already clears black.
    pub fn set_scene_blackout(&mut self, blackout: bool) {
        self.scene_blackout = blackout;
    }

    pub fn scene_size(&self) -> (u32, u32) {
        self.latest_ui()
            .and_then(|ui| ui.scene.as_ref())
            .map_or(self.size, |s| (s.rect[2] as u32, s.rect[3] as u32))
    }

    pub fn console_sizes(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
    ) -> anyhow::Result<[i32; 2]> {
        if self.console.is_none() {
            self.console = Some(crate::console_render::Renderer::with_pipeline(
                &gpu.device,
                &gpu.queue,
                crate::text_render_gpu::Pipeline::cached(
                    &self.pipelines,
                    &gpu.device,
                    gpu.config.format,
                ),
                pack,
            )?);
        }
        Ok(self.console.as_ref().unwrap().line_sizes())
    }

    pub fn prepare_console(
        &mut self,
        gpu: &crate::gpu_device::Device,
        console: &dyn crate::console_draw::ConsoleView,
        cycle: i32,
        focused: bool,
    ) -> anyhow::Result<()> {
        if let Some(ui) = &mut self.console {
            ui.prepare(
                &gpu.device,
                console,
                [self.size.0, self.size.1],
                cycle,
                focused,
            )?;
        }
        Ok(())
    }
}
