//! Interleaves interface-model batches with retained 2D quads in traversal
//! order.
use crate::{
    floor_render::{FloorMesh, FloorPipeline, FloorTextureCache, FloorUniforms, MeshUpload},
    interface_model::{Draw, DrawSpace},
};
use anyhow::Result;
use std::collections::HashMap;
struct Entry {
    owner: usize,
    mesh: FloorMesh,
    quad: usize,
    before: bool,
    clip: [u32; 4],
    /// Group-0 uniforms of this model's `drawParticles` and billboards
    /// (identity view).
    particles: Option<(wgpu::Buffer, wgpu::BindGroup)>,
}
/// The identity view interface models draw under.
const IDENTITY_VIEW: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

/// The faithful pipeline uniforms of one interface model draw (renderer plan
/// A4: the UI hands the neutral [`DrawSpace`], this toolkit derives its own
/// state): the model's (its draw matrix, the interface projection and the
/// interface lighting) and its particles' and billboards' (the projection
/// with the identity view, fog off). A pure function of the draw,
/// evaluated when the toolkit prepares it.
pub fn interface_uniforms(draw: &DrawSpace) -> (FloorUniforms, FloorUniforms) {
    let env = &draw.lighting;
    let projection = &draw.projection;
    let m = matrix4x3(&draw.matrix);
    let mut u = FloorUniforms::new(glam::Mat4::IDENTITY, env);
    let wvp = crate::camera::multiply(&m.entries(), projection);
    // Canvas +Y points down. The projection's Y row is flipped for top-left
    // surfaces; WGPU uses the same clip-space orientation.
    let flip = glam::Mat4::from_scale(glam::vec3(1., -1., 1.));
    u.wvp = (crate::camera::gl_to_wgpu_depth() * flip * crate::camera::to_glam(&wvp))
        .to_cols_array_2d();
    u.shadow_wvp = u.wvp;
    u.model_world = crate::camera::to_glam(&m.entries()).to_cols_array_2d();
    u.scene_origin = [0.; 4];
    u.scene_base = [0.; 4];
    let inv = m.inverse();
    let sun = inv.vector(env.sun.dir);
    u.sun_dir = [sun[0], sun[1], sun[2], 0.];
    u.eye_time = [inv.0[9], inv.0[10], inv.0[11], 0.];
    let mut particle_uniforms = u;
    let flip = glam::Mat4::from_scale(glam::vec3(1., -1., 1.));
    particle_uniforms.wvp =
        (crate::camera::gl_to_wgpu_depth() * flip * crate::camera::to_glam(projection))
            .to_cols_array_2d();
    // Particles fog only when the fog range is positive; the interface
    // lighting sets fog colour 13156520 with no range, and custom lighting
    // keeps it.
    particle_uniforms.distance_fog_plane = [0.; 4];
    (u, particle_uniforms)
}

/// The `Matrix4x3` whose `Matrix4x4` entries `entries` are
/// (`actor_matrix::Matrix::entries` copies the twelve floats; this copies
/// them back, bit for bit).
fn matrix4x3(entries: &[f32; 16]) -> crate::actor_matrix::Matrix {
    crate::actor_matrix::Matrix(std::array::from_fn(|i| entries[(i / 3) * 4 + i % 3]))
}
/// The colour and depth attachments interface models draw into.
#[derive(Clone, Copy)]
pub struct ModelTargets<'a> {
    pub view: &'a wgpu::TextureView,
    pub depth: &'a wgpu::TextureView,
}
pub struct ModelFrame {
    msaa: Option<crate::scene_target::Target>,
    pipeline: FloorPipeline,
    textures: FloorTextureCache,
    entries: Vec<Entry>,
    /// The particles of the interface models.
    particles: crate::particle_render::ParticlePass,
    particle_builder: crate::particle_render::Builder,
    /// Billboard quads of the interface models.
    billboards: crate::billboard_render::BillboardPass,
    /// Whether bloom is enabled (it hides flagged billboards).
    pub bloom: bool,
    format: wgpu::TextureFormat,
}
impl ModelFrame {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_pipelines(device, format, crate::pipelines::PipelineCache::default())
    }

    /// [`Models::new`] taking its pipelines from `pipelines` (the renderer's
    /// cache).
    pub fn with_pipelines(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        pipelines: crate::pipelines::PipelineCache,
    ) -> Self {
        let pipeline =
            FloorPipeline::with_cache(device, format, wgpu::TextureFormat::Depth24Plus, pipelines);
        let particles = crate::particle_render::ParticlePass::new(
            device,
            crate::particle_render::ParticlePipelines::cached(
                pipeline.cache(),
                device,
                &pipeline,
                format,
                pipeline.sample_count,
            ),
        );
        let billboards = crate::billboard_render::BillboardPass::new(
            device,
            crate::billboard_render::BillboardPipelines::cached(
                pipeline.cache(),
                device,
                &pipeline,
                format,
                pipeline.sample_count,
            ),
        );
        Self {
            msaa: None,
            pipeline,
            textures: FloorTextureCache::default(),
            entries: vec![],
            particles,
            particle_builder: crate::particle_render::Builder::default(),
            billboards,
            bloom: false,
            format,
        }
    }
    pub fn set_samples(
        &mut self,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        size: [u32; 2],
        samples: u32,
    ) {
        if self.pipeline.sample_count != samples || self.format != format {
            self.particles = crate::particle_render::ParticlePass::new(
                device,
                crate::particle_render::ParticlePipelines::cached(
                    self.pipeline.cache(),
                    device,
                    &self.pipeline,
                    format,
                    samples,
                ),
            );
            self.billboards = crate::billboard_render::BillboardPass::new(
                device,
                crate::billboard_render::BillboardPipelines::cached(
                    self.pipeline.cache(),
                    device,
                    &self.pipeline,
                    format,
                    samples,
                ),
            );
            self.format = format;
        }
        self.pipeline
            .set_sample_count(device, format, wgpu::TextureFormat::Depth24Plus, samples);
        if samples == 1 {
            self.msaa = None;
        } else if self.msaa.as_ref().is_none_or(|m| !m.matches(size, samples)) {
            self.msaa = Some(crate::scene_target::Target::with_pipelines(
                device,
                size,
                format,
                wgpu::TextureFormat::Depth24Plus,
                samples,
                crate::scene_target::CopyPipelines::cached(
                    self.pipeline.cache(),
                    device,
                    format,
                    samples,
                ),
            ));
        }
    }
    fn prepare_owned(
        &mut self,
        owners: &mut HashMap<usize, crate::interface_model::ModelOwner>,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        draws: Vec<Draw>,
        canvas: [u32; 2],
        target: [u32; 2],
    ) -> Result<()> {
        owners.retain(|_, owner| owner.strong_count() > 0);
        let mut old: HashMap<_, _> = std::mem::take(&mut self.entries)
            .into_iter()
            .filter(|e| owners.contains_key(&e.owner))
            .map(|e| (e.owner, e))
            .collect();
        let millis = self.pipeline.begin_material_frame();
        let mut material_state = crate::material::MaterialState::default();
        let mut particle_frame = crate::particle_render::Frame::default();
        let mut particle_resources = None;
        let mut billboard_frame = crate::billboard_render::Frame::default();
        let mut billboard_resources = None;
        for d in draws {
            let [l, t, r, b] = crate::ui_paint::framebuffer_bounds(d.clip, canvas, target);
            let [l, t, r, b] = [
                l.max(0),
                t.max(0),
                r.min(target[0] as i32),
                b.min(target[1] as i32),
            ];
            if r <= l || b <= t {
                continue;
            }
            let res = &d.resources;
            let (uniforms, particle_uniforms) = interface_uniforms(&d.space);
            self.pipeline.set_environment(
                device,
                queue.queue(),
                &res.pack,
                &res.materials,
                d.space.lighting.sampler,
                [0.; 3],
            )?;
            let mut mesh = if let Some(mut e) = old.remove(&(d.owner.as_ptr() as usize)) {
                if e.mesh.update_model(queue, &res.materials, &d.model)? {
                    e.mesh
                } else {
                    FloorMesh::from_model(
                        MeshUpload {
                            device,
                            queue: queue.queue(),
                            pipeline: &self.pipeline,
                            textures: &mut self.textures,
                            pack: &res.pack,
                            materials: &res.materials,
                        },
                        &d.model,
                        [0.; 3],
                        "UI player body",
                    )?
                }
            } else {
                FloorMesh::from_model(
                    MeshUpload {
                        device,
                        queue: queue.queue(),
                        pipeline: &self.pipeline,
                        textures: &mut self.textures,
                        pack: &res.pack,
                        materials: &res.materials,
                    },
                    &d.model,
                    [0.; 3],
                    "UI player body",
                )?
            };
            mesh.set_depth_write(d.depth_write);
            mesh.set_model_uniforms(device, queue, &self.pipeline, &uniforms);
            mesh.prepare_materials(queue, &mut material_state, millis, &[[0.; 4]; 8], 0);
            // The component's particles draw right after its model, in the
            // model's view space; its billboards follow the batches in the
            // same space, with the interface's fog off.
            let billboards_before = billboard_frame.draws.len();
            if let Some(b) = &mesh.billboards {
                let view = crate::billboard_render::View::new(
                    IDENTITY_VIEW,
                    &d.space.projection,
                    None,
                    self.bloom,
                );
                billboard_frame.add(0, self.entries.len(), b, &d.space.matrix, &view);
            }
            let has_billboards = billboard_frame.draws.len() > billboards_before;
            if has_billboards {
                billboard_resources = Some(d.resources.clone());
            }
            let particles = (!d.particle_list.is_empty() || has_billboards).then(|| {
                if !d.particle_list.is_empty() {
                    self.particle_builder.add_segment(
                        &mut particle_frame,
                        0,
                        self.entries.len(),
                        &d.particle_list,
                        &IDENTITY_VIEW,
                        [0; 3],
                    );
                    particle_resources = Some(d.resources.clone());
                }
                self.pipeline.uniform_group(device, &particle_uniforms)
            });
            let owner = d.owner.as_ptr() as usize;
            owners.insert(owner, d.owner);
            self.entries.push(Entry {
                owner,
                mesh,
                quad: d.quad,
                before: d.before_scene,
                clip: [l as u32, t as u32, (r - l) as u32, (b - t) as u32],
                particles,
            });
        }
        if let Some(res) = particle_resources {
            self.particles.prepare(
                device,
                queue,
                &mut self.textures,
                &res.pack,
                &res.materials,
                &particle_frame,
            )?;
        } else {
            self.particles.clear();
        }
        if let Some(res) = billboard_resources {
            self.billboards.prepare(
                device,
                queue,
                &mut self.textures,
                &res.pack,
                &res.materials,
                &billboard_frame,
            )?;
        } else {
            self.billboards.clear();
        }
        Ok(())
    }
    pub fn encode(
        &self,
        device: &wgpu::Device,
        paint: &impl crate::ui_paint_gpu::PaintDraw,
        split: usize,
        encoder: &mut wgpu::CommandEncoder,
        targets: ModelTargets<'_>,
        before: bool,
    ) {
        let ModelTargets { view, depth } = targets;
        let mut start = if before { 0 } else { split };
        let mut clear = before;
        for (index, e) in self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.before == before)
        {
            self.quads(paint, encoder, view, start..e.quad, clear);
            clear = false;
            if let Some(msaa) = &self.msaa {
                msaa.seed_from(device, encoder, view);
            }
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("UI player model"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.msaa.as_ref().map_or(view, |t| t.attachment()),
                    resolve_target: self.msaa.as_ref().map(|_| view),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self.msaa.as_ref().map_or(depth, |t| &t.depth),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_scissor_rect(e.clip[0], e.clip[1], e.clip[2], e.clip[3]);
            e.mesh.draw_model(&mut pass, &self.pipeline);
            if let Some((_, uniforms)) = &e.particles {
                self.billboards
                    .draw_at(&mut pass, uniforms, 0, index, usize::MAX);
                self.particles.draw_entity(&mut pass, uniforms, 0, index);
            }
            drop(pass);
            start = e.quad;
        }
        self.quads(
            paint,
            encoder,
            view,
            start..if before { split } else { usize::MAX },
            clear,
        );
    }
    fn quads(
        &self,
        paint: &impl crate::ui_paint_gpu::PaintDraw,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        range: std::ops::Range<usize>,
        clear: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("UI quads"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: if clear {
                        wgpu::LoadOp::Clear(wgpu::Color::BLACK)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        paint.draw_range(&mut pass, range);
    }
}

/// Main-thread cache owners beside the movable GPU frame.
pub struct Models {
    frame: Option<ModelFrame>,
    owners: HashMap<usize, crate::interface_model::ModelOwner>,
}
impl std::ops::Deref for Models {
    type Target = ModelFrame;
    fn deref(&self) -> &ModelFrame {
        self.frame.as_ref().expect("UI model frame is on main")
    }
}
impl std::ops::DerefMut for Models {
    fn deref_mut(&mut self) -> &mut ModelFrame {
        self.frame.as_mut().expect("UI model frame is on main")
    }
}
impl Models {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        Self::with_pipelines(device, format, crate::pipelines::PipelineCache::default())
    }
    pub fn with_pipelines(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        pipelines: crate::pipelines::PipelineCache,
    ) -> Self {
        Self {
            frame: Some(ModelFrame::with_pipelines(device, format, pipelines)),
            owners: HashMap::new(),
        }
    }
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        draws: Vec<Draw>,
        canvas: [u32; 2],
        target: [u32; 2],
    ) -> Result<()> {
        self.frame
            .as_mut()
            .expect("UI model frame is on main")
            .prepare_owned(&mut self.owners, device, queue, draws, canvas, target)
    }
    pub fn take_frame(&mut self) -> ModelFrame {
        self.frame.take().expect("UI frame handoff once")
    }
    pub fn restore_frame(&mut self, frame: ModelFrame) {
        assert!(self.frame.is_none(), "restore the matching UI frame");
        self.frame = Some(frame);
    }
}

#[cfg(any(test, feature = "test-hooks"))]
pub struct Capture {
    device: wgpu::Device,
    queue: wgpu::Queue,
    paint: crate::ui_paint_gpu::Renderer,
    models: Models,
    target: wgpu::Texture,
    view: wgpu::TextureView,
    depth: wgpu::TextureView,
    size: [u32; 2],
}
#[cfg(any(test, feature = "test-hooks"))]
impl Capture {
    pub fn new(size: [u32; 2]) -> Result<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .map_err(|err| anyhow::anyhow!("GPU adapter unavailable: {err}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("UI preview acceptance"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits {
                    max_texture_dimension_2d: size[0].max(size[1]).max(2048),
                    ..wgpu::Limits::downlevel_defaults()
                },
                memory_hints: Default::default(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                trace: wgpu::Trace::Off,
            }))?;
        let desc = wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        };
        let target = device.create_texture(&desc);
        let view = target.create_view(&Default::default());
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                format: wgpu::TextureFormat::Depth24Plus,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                ..desc
            })
            .create_view(&Default::default());
        let paint = crate::ui_paint_gpu::Renderer::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let models = Models::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        Ok(Self {
            device,
            queue,
            paint,
            models,
            target,
            view,
            depth,
            size,
        })
    }
    pub fn set_samples(&mut self, samples: u32) {
        self.models.set_samples(
            &self.device,
            wgpu::TextureFormat::Rgba8Unorm,
            self.size,
            samples,
        );
    }
    pub fn render(
        &mut self,
        out: crate::ui_output::Output<crate::interface_model::Draw>,
        path: &std::path::Path,
    ) -> Result<Vec<u8>> {
        let canvas = out.paint.size;
        let boundaries: Vec<_> = std::iter::once(out.scene_quad)
            .chain(out.models.iter().map(|d| d.quad))
            .collect();
        self.paint.prepare_boundaries_target(
            &self.device,
            &self.queue,
            out.paint,
            &boundaries,
            self.size,
        )?;
        self.models
            .prepare(&self.device, &self.queue, out.models, canvas, self.size)?;
        let mut encoder = self.device.create_command_encoder(&Default::default());
        let targets = ModelTargets {
            view: &self.view,
            depth: &self.depth,
        };
        self.models.encode(
            &self.device,
            &self.paint,
            out.scene_quad,
            &mut encoder,
            targets,
            true,
        );
        self.models.encode(
            &self.device,
            &self.paint,
            out.scene_quad,
            &mut encoder,
            targets,
            false,
        );
        let pitch = (self.size[0] * 4 + 255) & !255;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(pitch * self.size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(pitch),
                    rows_per_image: Some(self.size[1]),
                },
            },
            self.target.size(),
        );
        self.queue.submit(Some(encoder.finish()));
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv()??;
        let mapped = buffer.slice(..).get_mapped_range().expect("mapped range");
        let mut pixels: Vec<_> = mapped
            .chunks_exact(pitch as usize)
            .flat_map(|row| row[..self.size[0] as usize * 4].iter().copied())
            .collect();
        // The blend state intentionally writes alpha=0 for blended model faces.
        // The desktop surface is opaque; PNG alpha is presentation-only.
        for p in pixels.chunks_exact_mut(4) {
            p[3] = 255;
        }
        let rgb: Vec<_> = pixels
            .chunks_exact(4)
            .flat_map(|p| p[..3].iter().copied())
            .collect();
        crate::png_out::write_rgb_png(path, self.size[0], self.size[1], &rgb)?;
        Ok(pixels)
    }
}
