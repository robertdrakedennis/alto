//! Draw-only composition packets. Main owns the weak CPU cache registries and
//! alternates their GPU storage; the render thread owns encoding and presentation.
use super::frame::{copy_retained_canvas, note_frame_acquired, note_skipped_frame, retain_frame};
use super::*;

pub(super) struct LayerFrame {
    view: wgpu::TextureView,
    paint: crate::ui_paint_gpu::PaintFrame,
}
struct UiFrame {
    models: crate::ui_model_gpu::ModelFrame,
    paint: crate::ui_paint_gpu::PaintFrame,
    split: usize,
    layers: Vec<LayerFrame>,
}
struct CanvasFrame {
    texture: wgpu::Texture,
    presenter: crate::ui_paint_gpu::PaintFrame,
}

/// The exact GPU composition prepared for one scene frame, with no Rc/Weak
/// owners. It returns its interface-model storage to the matching main slot.
pub struct Composition {
    ui: Option<UiFrame>,
    console: Option<crate::console_render::Renderer>,
    canvas: Option<CanvasFrame>,
    depth: wgpu::TextureView,
    pub size: [u32; 2],
    pub viewport: Option<crate::frame_plan::Scene>,
    pub retained: Option<wgpu::Texture>,
    timer: Option<crate::frame_profile::PassTimer>,
}

impl Renderer {
    pub fn set_threaded_composition(&mut self, enabled: bool) {
        if !enabled && self.composition_pending {
            std::mem::swap(&mut self.ui, &mut self.ui_spare);
            self.composition_pending = false;
        }
        self.threaded_composition = enabled;
    }
    pub fn take_composition(&mut self) -> Composition {
        let viewport = self.scene_viewport();
        let ui = self.ui.as_mut().map(|ui| UiFrame {
            models: ui.models.take_frame(),
            paint: ui.paint.frame(),
            split: ui.split,
            layers: std::mem::take(&mut ui.layer_frames),
        });
        std::mem::swap(&mut self.ui, &mut self.ui_spare);
        self.composition_pending = true;
        Composition {
            ui,
            console: self.console.clone(),
            depth: self.depth_view.clone(),
            canvas: self.canvas_target.as_ref().map(|canvas| CanvasFrame {
                texture: canvas.texture.clone(),
                presenter: canvas.presenter.frame(),
            }),
            size: [self.size.0, self.size.1],
            viewport,
            retained: self.retained_frame.take(),
            timer: self.pass_timer.take(),
        }
    }
    pub fn take_present(&mut self, gpu: &crate::gpu_device::Device) -> Option<Composition> {
        let retained = self.retained_frame.as_ref()?;
        if retained.width() != self.size.0
            || retained.height() != self.size.1
            || retained.format() != gpu.config.format
            || (self.canvas_target.is_none()
                && !gpu.config.usage.contains(wgpu::TextureUsages::COPY_DST))
        {
            return None;
        }
        Some(Composition {
            ui: None,
            console: None,
            depth: self.depth_view.clone(),
            size: [self.size.0, self.size.1],
            viewport: None,
            canvas: self.canvas_target.as_ref().map(|canvas| CanvasFrame {
                texture: canvas.texture.clone(),
                presenter: canvas.presenter.frame(),
            }),
            retained: self.retained_frame.take(),
            timer: self.pass_timer.take(),
        })
    }
    pub(super) fn latest_ui(&self) -> Option<&RetainedUi> {
        if self.composition_pending {
            self.ui_spare.as_ref()
        } else {
            self.ui.as_ref()
        }
    }
    pub fn restore_composition(&mut self, composition: Composition) {
        if let Some(ui) = composition.ui {
            self.ui_spare
                .as_mut()
                .expect("matching UI frame owner")
                .models
                .restore_frame(ui.models);
        }
        self.retained_frame = composition.retained;
        self.pass_timer = composition.timer;
    }
}

pub(super) fn prepare_layers(
    gpu: &crate::gpu_device::Device,
    pipelines: &crate::pipelines::PipelineCache,
    ui: &mut RetainedUi,
    layers: Vec<(u64, crate::ui_paint::Plan)>,
    target: [u32; 2],
) -> anyhow::Result<Vec<(u64, wgpu::TextureView)>> {
    ui.layer_frames.clear();
    let active: std::collections::HashSet<_> = layers.iter().map(|(id, _)| *id).collect();
    ui.layer_targets.retain(|id, _| active.contains(id));
    ui.layer_painters.retain(|id, _| active.contains(id));
    let mut views = Vec::with_capacity(layers.len());
    for (id, plan) in layers {
        let size = wgpu::Extent3d {
            width: target[0],
            height: target[1],
            depth_or_array_layers: 1,
        };
        let texture = ui
            .layer_targets
            .entry(id)
            .or_insert_with(|| layer_texture(gpu, size));
        if texture.size() != size || texture.format() != gpu.config.format {
            *texture = layer_texture(gpu, size);
        }
        let paint = ui.layer_painters.entry(id).or_insert_with(|| {
            crate::ui_paint_gpu::Renderer::with_pipeline(crate::text_render_gpu::Pipeline::cached(
                pipelines,
                &gpu.device,
                gpu.config.format,
            ))
        });
        paint.prepare_split_target(&gpu.device, gpu, plan, None, target)?;
        ui.layer_frames.push(LayerFrame {
            view: texture.create_view(&Default::default()),
            paint: paint.frame(),
        });
        views.push((id, texture.create_view(&Default::default())));
    }
    Ok(views)
}
fn layer_texture(gpu: &crate::gpu_device::Device, size: wgpu::Extent3d) -> wgpu::Texture {
    gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("framebuffer sprite"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: gpu.config.format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    })
}

impl Composition {
    /// Encode the frozen framebuffer sprites and interface segments beneath
    /// the scene into a caller-owned target (also used by offscreen previews).
    pub fn encode_under(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        if let Some(ui) = &self.ui {
            for layer in &ui.layers {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("framebuffer sprite"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &layer.view,
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
                layer.paint.draw(&mut pass);
            }
            ui.models.encode(
                device,
                &ui.paint,
                ui.split,
                encoder,
                crate::ui_model_gpu::ModelTargets {
                    view,
                    depth: &self.depth,
                },
                true,
            );
        }
    }
    /// Encode the frozen interface segments above the scene and console.
    pub fn encode_over(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
    ) {
        if let Some(ui) = &self.ui {
            ui.models.encode(
                device,
                &ui.paint,
                ui.split,
                encoder,
                crate::ui_model_gpu::ModelTargets {
                    view,
                    depth: &self.depth,
                },
                false,
            );
        }
        if let Some(console) = &self.console {
            console.encode(encoder, view);
        }
    }
    fn present_canvas(&self, encoder: &mut wgpu::CommandEncoder, frame: &Frame) {
        if let Some(canvas) = &self.canvas {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("present bounded canvas"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame.surface_view,
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
            canvas.presenter.draw(&mut pass);
        }
    }
    pub fn present(&mut self, gpu: &mut crate::gpu_device::Device) -> anyhow::Result<()> {
        let Some(frame) = rs910_core::profile::scope!("frame acquire", self.acquire(gpu)?) else {
            return Ok(());
        };
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("retained present"),
            });
        let target = self
            .canvas
            .as_ref()
            .map_or(&frame.output.texture, |canvas| &canvas.texture);
        copy_retained_canvas(
            &mut encoder,
            self.retained.as_ref().expect("retained frame"),
            target,
        );
        self.present_canvas(&mut encoder, &frame);
        rs910_core::profile::scope!(
            "frame submit",
            Renderer::end_frame_with_commands(gpu, Vec::new(), encoder, frame)
        );
        Ok(())
    }

    fn acquire(&mut self, gpu: &mut crate::gpu_device::Device) -> anyhow::Result<Option<Frame>> {
        let output = match gpu.acquire() {
            wgpu::CurrentSurfaceTexture::Success(frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
            wgpu::CurrentSurfaceTexture::Lost | wgpu::CurrentSurfaceTexture::Outdated => {
                gpu.configure_surface();
                self.depth = Renderer::create_depth(&gpu.device, self.size[0], self.size[1]);
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
                return Err(anyhow::anyhow!("acquire surface texture: validation error"))
            }
        };
        note_frame_acquired();
        let surface_view = output.texture.create_view(&Default::default());
        let canvas_view = self
            .canvas
            .as_ref()
            .map(|canvas| canvas.texture.create_view(&Default::default()));
        Ok(Some(Frame {
            output,
            surface_view,
            canvas_view,
        }))
    }
    fn begin_timer(&mut self, gpu: &crate::gpu_device::Device) {
        if !rs910_core::profile::COMPILED {
            return;
        }
        let frame =
            rs910_core::profile::current_frame().filter(|_| rs910_core::profile::gpu_timing());
        if frame.is_some() && self.timer.is_none() {
            self.timer = crate::frame_profile::PassTimer::new(&gpu.device, &gpu.queue);
        }
        if let Some(timer) = &mut self.timer {
            timer.begin(&gpu.device, frame);
        }
    }
    fn mark(&mut self, encoder: &mut wgpu::CommandEncoder, name: &'static str) {
        if let Some(timer) = &mut self.timer {
            timer.mark(encoder, name);
        }
    }
    /// Acquire only after the caller has prepared the scene. False means no
    /// scene commands were encoded; the caller must restore producer progress.
    pub fn draw(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        scene: impl FnOnce(SceneTarget<'_>) -> Vec<wgpu::CommandBuffer>,
    ) -> anyhow::Result<bool> {
        let Some(frame) = rs910_core::profile::scope!("frame acquire", self.acquire(gpu)?) else {
            return Ok(false);
        };
        self.begin_timer(gpu);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("composite frame"),
            });
        self.mark(&mut encoder, "gpu ui under");
        self.encode_under(&gpu.device, &mut encoder, frame.view());
        self.mark(&mut encoder, "gpu scene (backend)");
        let commands = match self.viewport.as_ref() {
            Some(viewport) => scene(SceneTarget {
                device: &gpu.device,
                queue: &gpu.queue,
                encoder: &mut encoder,
                view: frame.view(),
                format: gpu.config.format,
                size: self.size,
                viewport: crate::frame_plan::Scene {
                    rect: viewport.rect,
                    clip: viewport.clip,
                },
            }),

            None => Vec::new(),
        };
        self.mark(&mut encoder, "gpu ui over");
        self.encode_over(&gpu.device, &mut encoder, frame.view());
        retain_frame(
            &gpu.device,
            &mut encoder,
            &mut self.retained,
            self.canvas
                .as_ref()
                .map_or(&frame.output.texture, |canvas| &canvas.texture),
        );
        self.present_canvas(&mut encoder, &frame);
        self.mark(&mut encoder, "");
        if let Some(timer) = &mut self.timer {
            timer.resolve(&mut encoder);
        }
        rs910_core::profile::scope!(
            "frame submit",
            Renderer::end_frame_with_commands(gpu, commands, encoder, frame)
        );
        if let Some(timer) = &mut self.timer {
            timer.submitted();
        }
        Ok(true)
    }
}
