//! The camera-facing quads of the interface models: their particles
//! (`rs910_scene::particle_quads`) and the billboards their meshes carry,
//! drawn right after each model with the model's identity-view uniforms
//! (`ui_model_gpu::interface_uniforms`; the floor pipeline's group 0).
//!
//! Both kinds share one shader, one quad vertex format and one material
//! table: `texture * vertex colour`, then distance fog from
//! `saturate(dot(vertex, distance_fog_plane))` (interface quads draw with a
//! zero plane, so unfogged). A quad blends with `SRC_ALPHA,
//! ONE_MINUS_SRC_ALPHA` and the alpha test `> 0`, or, with a nonzero alpha
//! reference, replaces with the alpha test `> ref`. Depth tests `LEQUAL`;
//! particles never write depth, billboards write it when their model has no
//! transparency.
//!
//! Per model, its billboards draw first (in mesh order, skipping the
//! bloom-hidden ones while bloom is on), then its particles.
use rs910_gpu_device::desc;

use std::collections::HashMap;
use std::ops::Range;

use wgpu::util::DeviceExt;

use crate::actor_matrix::Matrix;
use crate::billboard::QUAD_CORNERS;
use rs910_model::mesh_billboards::{culled, frustum_planes, MeshBillboards};
pub use rs910_scene::particle_quads::{Builder, ParticleFrame};
use rs910_scene::particle_quads::{ParticleVertex as QuadVertex, MAX_BATCH_QUADS};

const QUAD_SHADER: &str = r#"
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
struct QuadUniforms {
    alpha: vec4<f32>,   // x: alpha reference (GL_GREATER)
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> q: QuadUniforms;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var colour = textureSample(diffuse_tex, diffuse_sampler, in.uv) * in.colour;
    let fogged = vec4<f32>(u.distance_fog_colour.xyz, colour.w);
    colour = colour + in.fog * (fogged - colour);
    if (colour.w <= q.alpha.x) { discard; }
    return colour;
}
"#;

const VERTEX_ATTRS: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2];

/// Quad vertices per quad and indices per quad.
const QUAD_VERTICES: u16 = 4;
const QUAD_INDICES: u32 = 6;

/// `quads` consecutive quads from `first_vertex` sharing a material.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct QuadDraw {
    /// Material id (`-1` = no texture).
    material: i32,
    alpha_ref: u8,
    depth_write: bool,
    first_vertex: u32,
    quads: u32,
}

/// The billboard quads of one frame, each segment the tail of interface
/// model `at`.
#[derive(Clone, Debug, Default)]
pub struct BillboardFrame {
    vertices: Vec<QuadVertex>,
    draws: Vec<QuadDraw>,
    segments: Vec<(usize, Range<usize>)>,
}

/// The camera inputs of one frame's billboards.
#[derive(Clone, Debug)]
pub struct View {
    /// The view entries in the frame the meshes' world matrices use.
    view: [f32; 16],
    inverse: Matrix,
    planes: [[f32; 4]; 6],
    bloom: bool,
}

impl View {
    /// `view`/`projection` are 4x4 matrix entries; bloom hides the flagged
    /// billboards.
    #[must_use]
    pub fn new(view: [f32; 16], projection: &[f32; 16], bloom: bool) -> Self {
        let inverse = Matrix(std::array::from_fn(|i| view[i / 3 * 4 + i % 3])).inverse();
        let vp = crate::camera::multiply(&view, projection);
        Self {
            view,
            inverse,
            planes: frustum_planes(&vp),
            bloom,
        }
    }
}

impl BillboardFrame {
    /// The billboards of interface model `at`: `world` is its matrix in the
    /// view's frame.
    pub fn add(&mut self, at: usize, b: &MeshBillboards, world: &[f32; 16], view: &View) {
        if culled(b, world, &view.planes) {
            return;
        }
        let start = self.draws.len();
        for quad in b.placed_quads(world, &view.view, &view.inverse, view.bloom) {
            let first_vertex = self.vertices.len() as u32;
            for (pos, uv) in quad.corners.into_iter().zip(QUAD_CORNERS) {
                self.vertices.push(QuadVertex {
                    pos,
                    colour: quad.colour,
                    uv,
                });
            }
            self.draws.push(QuadDraw {
                material: quad.material,
                alpha_ref: b.alpha_ref,
                depth_write: !b.has_transparency,
                first_vertex,
                quads: 1,
            });
        }
        if self.draws.len() > start {
            self.segments.push((at, start..self.draws.len()));
        }
    }

    /// The number of billboard quads.
    #[must_use]
    pub fn len(&self) -> usize {
        self.draws.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.draws.is_empty()
    }
}

/// The quad pipelines by `(alpha tested, depth write)` and the material
/// layout, for one sample count and colour format
/// ([`crate::pipelines::Variant::Quads`]).
pub struct QuadPipelines {
    variants: [wgpu::RenderPipeline; 4],
    layout: wgpu::BindGroupLayout,
}

impl QuadPipelines {
    /// The one set for `(samples, format)` in `cache` (depth
    /// [`crate::pipelines::DEPTH_FORMAT`]).
    pub fn cached(
        cache: &crate::pipelines::PipelineCache,
        device: &wgpu::Device,
        floor: &crate::floor_render::FloorPipeline,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> std::sync::Arc<Self> {
        cache.get(samples, format, crate::pipelines::Variant::Quads, || {
            Self::new(device, floor, format, samples)
        })
    }

    fn new(
        device: &wgpu::Device,
        floor: &crate::floor_render::FloorPipeline,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("interface quads"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(QUAD_SHADER)),
        });
        let layout = desc::layout(
            device,
            "quad material",
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
            "quad layout",
            &[Some(floor.uniform_layout()), Some(&layout)],
        );
        let pipeline = |cutout: bool, depth_write: bool| {
            // Alpha blend + alpha test, or (cutout) alpha test only.
            let blend = (!cutout).then_some(wgpu::BlendState {
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
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("interface quads"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<QuadVertex>() as wgpu::BufferAddress,
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
                // Camera-facing quads: no culling.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(desc::depth_state(
                    crate::pipelines::DEPTH_FORMAT,
                    depth_write,
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
        Self {
            variants: [
                pipeline(false, false),
                pipeline(false, true),
                pipeline(true, false),
                pipeline(true, true),
            ],
            layout,
        }
    }

    fn get(&self, cutout: bool, depth_write: bool) -> &wgpu::RenderPipeline {
        &self.variants[usize::from(cutout) * 2 + usize::from(depth_write)]
    }
}

/// The interface models' quad draw owner.
pub struct QuadPass {
    pipelines: std::sync::Arc<QuadPipelines>,
    index_buffer: wgpu::Buffer,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: usize,
    bindings: HashMap<(i32, u8), wgpu::BindGroup>,
    draws: Vec<QuadDraw>,
    /// `(interface model, draws)` in draw order: billboards, then particles.
    segments: Vec<(usize, Range<usize>)>,
}

impl QuadPass {
    /// A pass (its quad indices and per-frame state) drawing with
    /// `pipelines` (`QuadPipelines::cached`).
    pub fn new(device: &wgpu::Device, pipelines: std::sync::Arc<QuadPipelines>) -> Self {
        let mut indices = Vec::with_capacity(MAX_BATCH_QUADS * QUAD_INDICES as usize);
        for quad in 0..MAX_BATCH_QUADS as u16 {
            let v = quad * QUAD_VERTICES;
            indices.extend_from_slice(&[v, v + 1, v + 2, v + 2, v + 3, v]);
        }
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("interface quads"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Self {
            pipelines,
            index_buffer,
            vertex_buffer: None,
            vertex_capacity: 0,
            bindings: HashMap::new(),
            draws: Vec::new(),
            segments: Vec::new(),
        }
    }

    /// Upload this frame's billboards and particles (particle segments of
    /// list 0 name their interface model) and resolve each draw's material
    /// through the shared material texture cache.
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        textures: &mut crate::floor_render::FloorTextureCache,
        resources: Option<&rs910_scene::interface_model::Resources>,
        billboards: BillboardFrame,
        particles: ParticleFrame,
    ) -> anyhow::Result<()> {
        self.draws.clear();
        self.segments.clear();
        let Some(resources) = resources else {
            return Ok(());
        };
        let BillboardFrame {
            mut vertices,
            mut draws,
            mut segments,
        } = billboards;
        let offset = vertices.len() as u32;
        let first_particle = draws.len();
        for batch in &particles.batches {
            // Particles take the alpha reference of alpha-tested materials.
            let alpha_ref = (batch.texture >= 0)
                .then(|| resources.materials.get(batch.texture as u32))
                .flatten()
                .filter(|m| m.alpha == crate::texture::AlphaMode::AlphaTested)
                .map_or(0, |m| m.alpha_threshold);
            draws.push(QuadDraw {
                material: batch.texture,
                alpha_ref,
                depth_write: false,
                first_vertex: offset + batch.first_vertex,
                quads: batch.quads,
            });
        }
        segments.extend(particles.segments.iter().filter(|s| s.list == 0).map(|s| {
            (
                s.entity,
                s.batches.start + first_particle..s.batches.end + first_particle,
            )
        }));
        vertices.extend_from_slice(&particles.vertices);
        if draws.is_empty() {
            return Ok(());
        }
        if self.vertex_capacity < vertices.len() {
            self.vertex_capacity = vertices.len().next_power_of_two();
            self.vertex_buffer = Some(desc::buffer(
                device,
                "interface quad vertices",
                (self.vertex_capacity * std::mem::size_of::<QuadVertex>()) as u64,
                wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            ));
        }
        queue.write_buffer(
            self.vertex_buffer.as_ref().expect("allocated above"),
            0,
            bytemuck::cast_slice(&vertices),
        );
        for draw in &draws {
            let key = (draw.material, draw.alpha_ref);
            if self.bindings.contains_key(&key) {
                continue;
            }
            // Without a diffuse map the white texture stands in.
            let material = (draw.material >= 0)
                .then(|| resources.materials.get(draw.material as u32))
                .flatten();
            let texture = textures.texture_or_white(
                device,
                queue.queue(),
                &resources.pack,
                &resources.materials,
                if material.is_some_and(|m| m.diffuse_texture.is_some()) {
                    draw.material
                } else {
                    -1
                },
            )?;
            let alpha = [f32::from(draw.alpha_ref) / 255.0, 0., 0., 0.];
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("quad material"),
                contents: bytemuck::cast_slice(&alpha),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind_group = desc::bind_group(
                device,
                "quad material",
                &self.pipelines.layout,
                &[
                    (0, wgpu::BindingResource::TextureView(texture.0)),
                    (1, wgpu::BindingResource::Sampler(texture.1)),
                    (2, buffer.as_entire_binding()),
                ],
            );
            self.bindings.insert(key, bind_group);
        }
        self.draws = draws;
        self.segments = segments;
        Ok(())
    }

    /// Draw the quads of interface model `model` with its group-0 uniforms.
    pub fn draw_model<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        uniforms: &'a wgpu::BindGroup,
        model: usize,
    ) {
        let Some(vertices) = self.vertex_buffer.as_ref() else {
            return;
        };
        let mut bound = false;
        for (_, range) in self.segments.iter().filter(|(at, _)| *at == model) {
            if !bound {
                pass.set_bind_group(0, uniforms, &[]);
                pass.set_vertex_buffer(0, vertices.slice(..));
                pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
                bound = true;
            }
            for d in &self.draws[range.clone()] {
                pass.set_pipeline(self.pipelines.get(d.alpha_ref != 0, d.depth_write));
                pass.set_bind_group(1, &self.bindings[&(d.material, d.alpha_ref)], &[]);
                pass.draw_indexed(0..d.quads * QUAD_INDICES, d.first_vertex as i32, 0..1);
            }
        }
    }
}
