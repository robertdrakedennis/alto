//! The two extra floor passes drawn after the material batches:
//!
//! 1. **Static point lights**: every baked light of the level draws its own
//!    `(pos, colour)` buffer with the standard model program (no point
//!    lights): ambient colour `(intensity,)*3`, the sun terms as left by the
//!    floor, the distance fog colour zeroed, blend `(DST_COLOR, ONE)`, depth
//!    writes off. The vertex declaration is position and colour only, so
//!    the normal is the GL default `(0, 0, 1)` and the lighting term is
//!    `intensity + sat(sunDir.z) * sunColour + sat(-sunDir.z) *
//!    antiSunColour`, mirrored here on purpose.
//! 2. **Floor hard shadows**: the floor geometry is drawn again per
//!    128x128-texel block with the unlit program (lighting = 1), the block's
//!    alpha texture (set texels 68, unset ones `17 x` set 4-neighbours),
//!    a texture-coordinate scale of `1 / (texel scale * 128)` translated by
//!    `(-blockX, -blockZ)`, the base colour stream (alpha 255), the model
//!    matrix lowered by one unit (`y -= 1`), and blend `(SRC_ALPHA,
//!    ONE_MINUS_SRC_ALPHA)` with alpha test `> 0`. An alpha-only texture
//!    samples as `(0, 0, 0, a)`, so the pass only darkens.

use wgpu::util::DeviceExt;

pub use crate::draw::{light_visible, shadow_indices};
use crate::floor::FloorGeometry;
use crate::floor_render::{FloorPipeline, FloorVertex};
use crate::floorlight::BakedLight;
pub use crate::hardshadow::ShadowMaskView;

/// WGSL for pass 1 (the standard model shading, no point lights, default
/// normal, fog colour black).
pub const LIGHT_SHADER: &str = r#"
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
    shadow_wvp: mat4x4<f32>,
};
struct LightUniforms {
    origin: vec4<f32>,     // scene base in fine units
    intensity: vec4<f32>,  // the light's intensity x3, 0
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(1) @binding(0) var<uniform> l: LightUniforms;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
};
struct VsOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) lighting: vec3<f32>,
    @location(2) fog: f32,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let vertex = vec4<f32>(v.pos, 1.0) + vec4<f32>(l.origin.xyz, 0.0);
    let local_vertex = vec4<f32>(v.pos + (l.origin.xyz - u.scene_origin.xyz), 1.0);
    out.clip = u.wvp * local_vertex;
    // Normal = (0, 0, 1): the position + colour declaration carries no
    // normals.
    let ndotl = u.sun_dir.z;
    let lit = clamp(ndotl, 0.0, 1.0);
    let anti = clamp(-ndotl, 0.0, 1.0);
    out.lighting = l.intensity.xyz + lit * u.sun_colour.xyz + anti * u.anti_sun_colour.xyz;
    out.fog = clamp(dot(vertex, u.distance_fog_plane), 0.0, 1.0);
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // The diffuse texture is the 1x1 white texture.
    var c = in.colour;
    c = vec4<f32>(c.xyz * in.lighting, c.w);
    // Height fog plane is zero here; distance fog colour is (0,0,0).
    c = vec4<f32>(c.xyz * (1.0 - in.fog), c.w);
    if (c.w <= 0.0) { discard; }
    return c;
}
"#;

/// WGSL for pass 2 (the unlit shading).
pub const SHADOW_SHADER: &str = r#"
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
    shadow_wvp: mat4x4<f32>,
};
struct BlockUniforms {
    origin: vec4<f32>,   // scene base in fine units
    block: vec4<f32>,    // (blockX, blockZ, 1 / (texel scale * 128), 0)
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(1) @binding(0) var shadow_tex: texture_2d<f32>;
@group(1) @binding(1) var shadow_sampler: sampler;
@group(1) @binding(2) var<uniform> b: BlockUniforms;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) depth: f32,
    @location(3) normal: vec3<f32>,
    @location(4) colour: vec4<f32>,
};
struct VsOut {
    @invariant @builtin(position) clip: vec4<f32>,
    @location(0) colour: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) fog: vec2<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let vertex = vec4<f32>(v.pos, 1.0) + vec4<f32>(b.origin.xyz, 0.0);
    // The one-unit lowering is composed into the transform on the CPU, not
    // added to an absolute-world vertex in the shader.
    let local_vertex = vec4<f32>(v.pos + (b.origin.xyz - u.scene_origin.xyz), 1.0);
    out.clip = u.shadow_wvp * local_vertex;
    out.uv = v.uv * b.block.z - b.block.xy;
    out.fog = vec2<f32>(
        clamp(dot(vertex, u.height_fog_plane), 0.0, 1.0),
        clamp(dot(vertex, u.distance_fog_plane), 0.0, 1.0),
    );
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    // Alpha-only texture: rgb 0, alpha from the mask.
    let a = textureSample(shadow_tex, shadow_sampler, in.uv).r;
    var diffuse = vec4<f32>(0.0, 0.0, 0.0, a) * in.colour;
    // Unlit: lighting = 1.
    let hf = vec4<f32>(u.height_fog_colour.xyz, diffuse.w);
    diffuse = diffuse + in.fog.x * (hf - diffuse);
    let df = vec4<f32>(u.distance_fog_colour.xyz, diffuse.w);
    diffuse = diffuse + in.fog.y * (df - diffuse);
    if (diffuse.w <= 0.0) { discard; }
    return diffuse;
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LightUniforms {
    origin: [f32; 4],
    intensity: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct BlockUniforms {
    origin: [f32; 4],
    block: [f32; 4],
}

const LIGHT_VERTEX_ATTRS: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4];
const SHADOW_VERTEX_ATTRS: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32x2,
    2 => Float32,
    3 => Float32x3
];
const SHADOW_COLOUR_ATTRS: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![4 => Unorm8x4];

/// Pipelines for both passes; they bind the floor's group-0 uniforms.
pub struct FloorPasses {
    pub light_pipeline: wgpu::RenderPipeline,
    light_layout: wgpu::BindGroupLayout,
    pub shadow_pipeline: wgpu::RenderPipeline,
    shadow_layout: wgpu::BindGroupLayout,
    shadow_sampler: wgpu::Sampler,
}

impl FloorPasses {
    /// The one set for `(samples, format)` in `cache` (depth
    /// [`crate::pipelines::DEPTH_FORMAT`]).
    pub fn cached(
        cache: &crate::pipelines::PipelineCache,
        device: &wgpu::Device,
        floor: &FloorPipeline,
        format: wgpu::TextureFormat,
        samples: u32,
    ) -> std::sync::Arc<Self> {
        cache.get(
            samples,
            format,
            crate::pipelines::Variant::FloorPasses,
            || {
                Self::with_samples(
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
        floor: &FloorPipeline,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
    ) -> Self {
        Self::with_samples(device, floor, surface_format, depth_format, 1)
    }
    pub fn with_samples(
        device: &wgpu::Device,
        floor: &FloorPipeline,
        surface_format: wgpu::TextureFormat,
        depth_format: wgpu::TextureFormat,
        sample_count: u32,
    ) -> Self {
        let light_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor static lights (Model.glsl ShaderMode 0, default normal)"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(LIGHT_SHADER)),
        });
        let light_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor light"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let light_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor light layout"),
            bind_group_layouts: &[Some(floor.uniform_layout()), Some(&light_layout)],
            immediate_size: 0,
        });
        let light_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor static lights"),
            layout: Some(&light_pl),
            vertex: wgpu::VertexState {
                module: &light_shader,
                entry_point: Some("vs_main"),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 16,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &LIGHT_VERTEX_ATTRS,
                })],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &light_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    // glBlendFunc(GL_DST_COLOR, GL_ONE): dst = src * dst + dst.
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Dst,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::Zero,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });

        let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("floor hard shadows (Model.glsl ShaderMode 3)"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(SHADOW_SHADER)),
        });
        let shadow_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("floor shadow block"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let shadow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("floor shadow layout"),
            bind_group_layouts: &[Some(floor.uniform_layout()), Some(&shadow_layout)],
            immediate_size: 0,
        });
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("floor hard shadows"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState {
                module: &shadow_shader,
                entry_point: Some("vs_main"),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<FloorVertex>() as wgpu::BufferAddress,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &SHADOW_VERTEX_ATTRS,
                    }),
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 4,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &SHADOW_COLOUR_ATTRS,
                    }),
                ],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shadow_shader,
                entry_point: Some("fs_main"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: surface_format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::SrcAlpha,
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::Zero,
                            dst_factor: wgpu::BlendFactor::Zero,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: depth_format,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: sample_count,
                ..Default::default()
            },
            multiview_mask: None,
            cache: None,
        });
        // Linear filtering both ways, no mipmaps, clamped edges.
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("floor shadow block"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        Self {
            light_pipeline,
            light_layout,
            shadow_pipeline,
            shadow_layout,
            shadow_sampler,
        }
    }
}

/// One uploaded static light.
pub struct LightMesh {
    pub source_light: Option<usize>,
    uniforms: wgpu::Buffer,
    vertex_buffer: wgpu::Buffer,
    index_buffer: wgpu::Buffer,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    bounds: [i32; 4],
    visible: bool,
}

impl LightMesh {
    /// Upload the light's streams. Returns `None` when the light produced
    /// no geometry.
    pub fn build(
        device: &wgpu::Device,
        passes: &FloorPasses,
        light: &BakedLight,
        origin_fine: [f32; 3],
        label: &str,
    ) -> Option<Self> {
        if light.indices.is_empty() || light.vertices.is_empty() {
            return None;
        }
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: &light.vertices,
            usage: wgpu::BufferUsages::VERTEX,
        });
        let mut idx = light.indices.clone();
        if idx.len() % 2 == 1 {
            idx.push(0);
        }
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(&idx),
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        });
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::bytes_of(&LightUniforms {
                origin: [origin_fine[0], origin_fine[1], origin_fine[2], 0.0],
                intensity: [light.intensity, light.intensity, light.intensity, 0.0],
            }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(label),
            layout: &passes.light_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniforms.as_entire_binding(),
            }],
        });
        Some(Self {
            source_light: light.source_light,
            uniforms,
            vertex_buffer,
            index_buffer,
            index_count: light.indices.len() as u32,
            bind_group,
            bounds: [light.tile_x0, light.tile_x1, light.tile_z0, light.tile_z1],
            visible: true,
        })
    }

    /// Set the light's live intensity (the same light value models read).
    pub fn set_intensity(&self, queue: &dyn crate::uploads::Uploader, value: f32) {
        queue.write_buffer(
            &self.uniforms,
            16,
            bytemuck::cast_slice(&[value, value, value, 0.]),
        );
    }

    /// Apply the tile selection: the light draws its full buffer or nothing
    /// (the partial branch's temporary index buffer is ignored on purpose).
    pub fn select_tiles(
        &mut self,
        geometry: &FloorGeometry,
        selection: &crate::draw::FloorSelection,
    ) {
        self.visible = light_visible(self.bounds, geometry, selection);
    }

    /// Draw the full buffer after the visibility gate.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        if !self.visible {
            return;
        }
        pass.set_bind_group(1, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        pass.draw_indexed(0..self.index_count, 0, 0..1);
    }
}

/// One hard-shadow block of the floor.
struct ShadowBlock {
    index_buffer: wgpu::Buffer,
    index_count: u32,
    bind_group: wgpu::BindGroup,
    bounds: [usize; 4],
    texture: wgpu::Texture,
    block: (usize, usize),
    hash: i32,
    dirty: bool,
}

/// The shadow pass geometry of one level (`FloorHardShadows` + its blocks).
pub struct ShadowMesh {
    vertex_buffer: wgpu::Buffer,
    colour_buffer: wgpu::Buffer,
    blocks: Vec<ShadowBlock>,
}

/// A wrapping row-count hash of a block's mask; it controls uploads.
pub fn block_hash(mask: &ShadowMaskView<'_>, bx: usize, bz: usize) -> i32 {
    let mut hash = 0i32;
    for z in 0..128 {
        hash = hash.wrapping_mul(255);
        let at = (bz * 128 + 1 + z) * mask.width + bx * 128 + 1;
        for &v in &mask.mask[at..at + 128] {
            hash = hash.wrapping_add(i32::from(v != 0));
        }
    }
    hash
}

/// The 128x128 alpha bytes of block
/// `(bx, bz)` — set texels 68, unset ones `17 x` the number of set
/// 4-neighbours (read from the global mask, border included).
#[must_use]
pub fn block_alpha(mask: &ShadowMaskView<'_>, bx: usize, bz: usize) -> Vec<u8> {
    let w = mask.width;
    let mut out = Vec::with_capacity(128 * 128);
    let origin = (bz * 128 + 1) * w + bx * 128 + 1;
    for row in 0..128 {
        let start = origin + row * w;
        for i in start..start + 128 {
            let v = mask.mask[i];
            if v == 0 {
                let mut n = 0_u8;
                if mask.mask[i - 1] != 0 {
                    n += 1;
                }
                if mask.mask[i + 1] != 0 {
                    n += 1;
                }
                if mask.mask[i - w] != 0 {
                    n += 1;
                }
                if mask.mask[i + w] != 0 {
                    n += 1;
                }
                out.push(n * 17);
            } else {
                out.push(68);
            }
        }
    }
    out
}

impl ShadowMesh {
    /// Build the shadow blocks and their index buffers: one block per
    /// `1 << (texel_shift + 7 - 9)` tiles, skipping blocks without floor
    /// triangles.
    pub fn build(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        passes: &FloorPasses,
        geometry: &FloorGeometry,
        mask: &ShadowMaskView<'_>,
        origin_fine: [f32; 3],
        label: &str,
    ) -> Self {
        let stride = geometry.stride_floats;
        let mut verts = Vec::with_capacity(geometry.vertex_count);
        for i in 0..geometry.vertex_count {
            let f = &geometry.stream0[i * stride..(i + 1) * stride];
            verts.push(FloorVertex {
                pos: [f[0], f[1], f[2]],
                uv: [f[3], f[4]],
                depth: 0.0,
                normal: [0.0, -1.0, 0.0],
            });
        }
        let pad = [FloorVertex {
            pos: [0.0; 3],
            uv: [0.0; 2],
            depth: 0.0,
            normal: [0.0, -1.0, 0.0],
        }];
        let vertex_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(if verts.is_empty() {
                &pad
            } else {
                verts.as_slice()
            }),
            usage: wgpu::BufferUsages::VERTEX,
        });
        // The base colour stream (alpha 255).
        let colours: Vec<u32> = geometry.base_colours.iter().map(|&c| c as u32).collect();
        let colour_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(label),
            contents: bytemuck::cast_slice(if colours.is_empty() {
                &[0_u32][..]
            } else {
                &colours
            }),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let block_shift = mask.texel_shift as usize + 7 - geometry.heights.shift as usize;
        let blocks_x = geometry.tiles_x >> block_shift;
        let blocks_z = geometry.tiles_z >> block_shift;
        let tiles_per_block = 1_usize << block_shift;
        let uv_scale = 1.0 / ((1_u32 << mask.texel_shift) as f32 * 128.0);
        let mut blocks = Vec::new();
        for bz in 0..blocks_z {
            for bx in 0..blocks_x {
                let mut indices: Vec<u16> = Vec::new();
                for tz in bz * tiles_per_block..(bz + 1) * tiles_per_block {
                    for tx in bx * tiles_per_block..(bx + 1) * tiles_per_block {
                        if let Some(Some(tris)) = geometry.tile_tris.get(geometry.tiles_x * tz + tx)
                        {
                            indices.extend_from_slice(tris);
                        }
                    }
                }
                if indices.is_empty() {
                    continue;
                }
                let alpha = block_alpha(mask, bx, bz);
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: 128,
                        height: 128,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d::ZERO,
                        aspect: wgpu::TextureAspect::All,
                    },
                    &alpha,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(128),
                        rows_per_image: Some(128),
                    },
                    wgpu::Extent3d {
                        width: 128,
                        height: 128,
                        depth_or_array_layers: 1,
                    },
                );
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::bytes_of(&BlockUniforms {
                        origin: [origin_fine[0], origin_fine[1], origin_fine[2], 0.0],
                        block: [bx as f32, bz as f32, uv_scale, 0.0],
                    }),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some(label),
                    layout: &passes.shadow_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(&view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(&passes.shadow_sampler),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: uniforms.as_entire_binding(),
                        },
                    ],
                });
                let mut idx = indices.clone();
                if idx.len() % 2 == 1 {
                    idx.push(0);
                }
                let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents: bytemuck::cast_slice(&idx),
                    usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                });
                blocks.push(ShadowBlock {
                    index_buffer,
                    index_count: indices.len() as u32,
                    bind_group,
                    bounds: [
                        bx * tiles_per_block,
                        (bx + 1) * tiles_per_block,
                        bz * tiles_per_block,
                        (bz + 1) * tiles_per_block,
                    ],
                    texture,
                    block: (bx, bz),
                    hash: block_hash(mask, bx, bz),
                    dirty: false,
                });
            }
        }
        Self {
            vertex_buffer,
            colour_buffer,
            blocks,
        }
    }

    #[must_use]
    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    pub fn update_mask(
        &mut self,
        queue: &wgpu::Queue,
        mask: &ShadowMaskView<'_>,
        dirty: &std::collections::HashSet<(usize, usize)>,
    ) {
        for block in &mut self.blocks {
            block.dirty |= dirty.contains(&block.block);
            // Dirty texture work is delayed until this block actually draws.
            if !block.dirty || block.index_count == 0 {
                continue;
            }
            block.dirty = false;
            let (bx, bz) = block.block;
            let hash = block_hash(mask, bx, bz);
            if hash == block.hash {
                continue;
            }
            block.hash = hash;
            let alpha = block_alpha(mask, bx, bz);
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &block.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                &alpha,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(128),
                    rows_per_image: Some(128),
                },
                wgpu::Extent3d {
                    width: 128,
                    height: 128,
                    depth_or_array_layers: 1,
                },
            );
        }
    }

    pub fn select_tiles(
        &mut self,
        queue: &dyn crate::uploads::Uploader,
        geometry: &FloorGeometry,
        selection: &crate::draw::FloorSelection,
    ) {
        for block in &mut self.blocks {
            let mut indices = shadow_indices(block.bounds, geometry, selection);
            block.index_count = indices.len() as u32;
            if !indices.len().is_multiple_of(2) {
                indices.push(0);
            }
            if !indices.is_empty() {
                queue.write_buffer(&block.index_buffer, 0, bytemuck::cast_slice(&indices));
            }
        }
    }

    /// Draw the selected blocks in Z/X order.
    pub fn draw<'a>(&'a self, pass: &mut wgpu::RenderPass<'a>) {
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, self.colour_buffer.slice(..));
        for block in &self.blocks {
            if block.index_count == 0 {
                continue;
            }
            pass.set_bind_group(1, &block.bind_group, &[]);
            pass.set_index_buffer(block.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
            pass.draw_indexed(0..block.index_count, 0, 0..1);
        }
    }
}

/// Everything one level draws: the floor batches, then its lights, then its
/// shadow blocks.
pub struct FloorLevelDraw<'a> {
    pub floor: &'a crate::floor_render::FloorMesh,
    pub lights: &'a [LightMesh],
    pub shadows: Option<&'a ShadowMesh>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_alpha_softens_edges_by_set_neighbours() {
        // 130x130 mask (one 128 block + border), a 2x2 set square at (10,10).
        let w = 130;
        let mut mask = vec![0_u8; w * w];
        for z in 11..13 {
            for x in 11..13 {
                mask[z * w + x] = 1;
            }
        }
        let view = ShadowMaskView {
            width: w,
            height: w,
            mask: &mask,
            texel_shift: 5,
        };
        let a = block_alpha(&view, 0, 0);
        // Block texel (col, row) = mask (col + 1, row + 1).
        assert_eq!(a[10 * 128 + 10], 68);
        assert_eq!(a[11 * 128 + 11], 68);
        // Left neighbour of the square: one set neighbour -> 17.
        assert_eq!(a[10 * 128 + 9], 17);
        // Diagonal corner: no 4-neighbour set -> 0.
        assert_eq!(a[9 * 128 + 9], 0);
    }
}
