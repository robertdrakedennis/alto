//! The particle pass of the interface models: the frame's particle quads
//! (`rs910_scene::particle_quads`) drawn through the floor pipeline's
//! group-0 camera/fog uniforms, depth test `LEQUAL` with depth writes off,
//! `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` with the alpha test `> 0`, or the
//! replace path with the material threshold for alpha-tested materials.
use rs910_gpu_device::desc;

use std::collections::HashMap;

use wgpu::util::DeviceExt;

pub use rs910_scene::particle_quads::{
    Builder, ParticleFrame as Frame, ParticleSegment as Segment, ParticleVertex, MAX_BATCH_QUADS,
};

/// The particle shader: `texture * vertex colour`, then distance fog from
/// `saturate(dot(vertex, distance_fog_plane))`; a disabled fog has a zero
/// plane, which draws unfogged.
pub const PARTICLE_SHADER: &str = r#"
struct FloorUniforms {
    wvp: mat4x4<f32>,
    sun_dir: vec4<f32>,
    sun_colour: vec4<f32>,
    anti_sun_colour: vec4<f32>,
    ambient_colour: vec4<f32>,
    height_fog_plane: vec4<f32>,
    height_fog_colour: vec4<f32>,
    distance_fog_plane: vec4<f32>,
    distance_fog_colour: vec4<f32>,
    scene_origin: vec4<f32>,
};
struct BatchUniforms {
    alpha: vec4<f32>,   // x: alpha reference (GL_GREATER)
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> b: BatchUniforms;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    out.clip = u.wvp * vec4<f32>(v.pos, 1.0);
    let vertex = vec4<f32>(v.pos + u.scene_origin.xyz, 1.0);
    out.fog = clamp(dot(vertex, u.distance_fog_plane), 0.0, 1.0);
    out.colour = v.colour;
    out.uv = v.uv;
    return out;
}

fn shade(in: VsOut) -> vec4<f32> {
    var diffuse = textureSample(diffuse_tex, diffuse_sampler, in.uv) * in.colour;
    let df = vec4<f32>(u.distance_fog_colour.xyz, diffuse.w);
    return diffuse + in.fog * (df - diffuse);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let diffuse = shade(in);
    if (diffuse.w <= b.alpha.x) { discard; }
    return diffuse;
}
"#;

const VERTEX_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2];

struct MaterialBinding {
    bind_group: wgpu::BindGroup,
    cutout: bool,
}

/// The particle pass's pipelines and material layout for one sample count
/// and colour format ([`crate::pipelines::Variant::Particles`]).
pub struct ParticlePipelines {
    blend_pipeline: wgpu::RenderPipeline,
    cutout_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
}

/// The scene-pass particle draw owner.
pub struct ParticlePass {
    pipelines: std::sync::Arc<ParticlePipelines>,
    index_buffer: wgpu::Buffer,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: usize,
    materials: HashMap<i32, MaterialBinding>,
    draws: Vec<(i32, u32, u32)>,
    segments: Vec<Segment>,
}

impl ParticlePipelines {
    /// The one set for `(samples, format)` in `cache` (depth
    /// [`crate::pipelines::DEPTH_FORMAT`]).
    pub fn cached(
        cache: &crate::pipelines::PipelineCache,
        device: &wgpu::Device,
        floor: &crate::floor_render::FloorPipeline,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> std::sync::Arc<Self> {
        cache.get(
            samples,
            format,
            crate::pipelines::Variant::Particles,
            || {
                Self::new(
                    device,
                    floor,
                    format,
                    crate::pipelines::DEPTH_FORMAT,
                    samples,
                )
            },
        )
    }

    pub fn new(
        device: &wgpu::Device,
        floor: &crate::floor_render::FloorPipeline,
        format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("particles (Particle.glsl)"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(PARTICLE_SHADER)),
        });
        let layout = desc::layout(
            device,
            "particle material",
            &[
                desc::float_texture(
                    0,
                    wgpu::ShaderStages::FRAGMENT,
                    true,
                    wgpu::TextureViewDimension::D2,
                ),
                desc::sampler(
                    1,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::SamplerBindingType::Filtering,
                ),
                desc::uniform(2, wgpu::ShaderStages::FRAGMENT),
            ],
        );
        let pipeline_layout = desc::pipeline_layout(
            device,
            "particle layout",
            &[Some(floor.uniform_layout()), Some(&layout)],
        );
        let pipeline = |label: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<ParticleVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &VERTEX_ATTRS,
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                // Camera-facing quads: the scene's cull state does not apply.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(desc::depth_state(
                    depth_format,
                    false,
                    wgpu::CompareFunction::LessEqual,
                )),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let blend_pipeline = pipeline(
            "particles blend",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                // As the floor pipeline's blend state.
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::Zero,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        );
        let cutout_pipeline = pipeline("particles alpha test", None);
        Self {
            blend_pipeline,
            cutout_pipeline,
            layout,
        }
    }
}

impl ParticlePass {
    /// A pass (its quad indices and per-frame state) drawing with
    /// `pipelines` (`ParticlePipelines::cached`).
    pub fn new(device: &wgpu::Device, pipelines: std::sync::Arc<ParticlePipelines>) -> Self {
        let mut indices = Vec::with_capacity(MAX_BATCH_QUADS * 6);
        for quad in 0..MAX_BATCH_QUADS as u16 {
            let v = quad * 4;
            indices.extend_from_slice(&[v, v + 1, v + 2, v + 2, v + 3, v]);
        }
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("particle quads"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipelines,
            index_buffer,
            vertex_buffer: None,
            vertex_capacity: 0,
            materials: HashMap::new(),
            draws: Vec::new(),
            segments: Vec::new(),
        }
    }

    /// Upload this frame's quads and resolve each batch's material through
    /// the shared material texture cache.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        textures: &mut crate::floor_render::FloorTextureCache,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        frame: &Frame,
    ) -> anyhow::Result<()> {
        self.draws.clear();
        self.segments.clear();
        if frame.batches.is_empty() {
            return Ok(());
        }
        self.segments = if frame.segments.is_empty() {
            vec![Segment {
                list: 1,
                entity: usize::MAX,
                batches: 0..frame.batches.len(),
            }]
        } else {
            frame.segments.clone()
        };
        if self.vertex_capacity < frame.vertices.len() {
            self.vertex_capacity = frame.vertices.len().next_power_of_two();
            self.vertex_buffer = Some(desc::buffer(
                device,
                "particle vertices",
                (self.vertex_capacity * std::mem::size_of::<ParticleVertex>()) as u64,
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            ));
        }
        queue.write_buffer(
            self.vertex_buffer.as_ref().unwrap(),
            0,
            bytemuck::cast_slice(&frame.vertices),
        );
        for batch in &frame.batches {
            if !self.materials.contains_key(&batch.texture) {
                // The material texture when it has one, alpha reference from
                // alpha-tested materials.
                let material = (batch.texture >= 0)
                    .then(|| materials.get(batch.texture as u32))
                    .flatten();
                let threshold = material
                    .filter(|m| m.alpha == crate::texture::AlphaMode::AlphaTested)
                    .map_or(0, |m| m.alpha_threshold);
                let texture = textures.texture_or_white(
                    device,
                    queue.queue(),
                    pack,
                    materials,
                    if material.is_some_and(|m| m.diffuse_texture.is_some()) {
                        batch.texture
                    } else {
                        -1
                    },
                )?;
                let alpha = [f32::from(threshold) / 255.0, 0., 0., 0.];
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("particle batch"),
                    contents: bytemuck::cast_slice(&alpha),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind_group = desc::bind_group(
                    device,
                    "particle material",
                    &self.pipelines.layout,
                    &[
                        (0, wgpu::BindingResource::TextureView(texture.0)),
                        (1, wgpu::BindingResource::Sampler(texture.1)),
                        (2, buffer.as_entire_binding()),
                    ],
                );
                self.materials.insert(
                    batch.texture,
                    MaterialBinding {
                        bind_group,
                        cutout: threshold != 0,
                    },
                );
            }
            self.draws
                .push((batch.texture, batch.first_vertex, batch.quads));
        }
        Ok(())
    }

    /// No particles this frame.
    pub fn clear(&mut self) {
        self.draws.clear();
        self.segments.clear();
    }

    #[must_use]
    pub fn has_draws(&self) -> bool {
        !self.draws.is_empty()
    }

    /// Draw the segments of plan entity `entity` of `list`.
    pub fn draw_entity<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        uniforms: &'a wgpu::BindGroup,
        list: u8,
        entity: usize,
    ) {
        for segment in &self.segments {
            if segment.list == list && segment.entity == entity {
                self.draw_batches(pass, uniforms, segment.batches.clone());
            }
        }
    }

    /// Draw every prepared batch in order with the scene's group-0 uniforms.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>, uniforms: &'a wgpu::BindGroup) {
        self.draw_batches(pass, uniforms, 0..self.draws.len());
    }

    fn draw_batches<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        uniforms: &'a wgpu::BindGroup,
        batches: std::ops::Range<usize>,
    ) {
        let Some(vertices) = self.vertex_buffer.as_ref() else {
            return;
        };
        let Some(draws) = self.draws.get(batches) else {
            return;
        };
        if draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, uniforms, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        let mut cutout = None;
        for &(texture, first, quads) in draws {
            let material = &self.materials[&texture];
            if cutout != Some(material.cutout) {
                pass.set_pipeline(if material.cutout {
                    &self.pipelines.cutout_pipeline
                } else {
                    &self.pipelines.blend_pipeline
                });
                cutout = Some(material.cutout);
            }
            pass.set_bind_group(1, &material.bind_group, &[]);
            pass.draw_indexed(0..quads * 6, first as i32, 0..1);
        }
    }
}
