//! The wgpu half of `text_render` (the batch shader and texture uploads):
//! the pipeline, textures and meshes
//! the retained-UI painter (`ui_paint_gpu`) draws the toolkit's quads with.
//! Split out in Phase 3.1; the CPU quad geometry (`Vertex`, `quad`, `glyph`)
//! is rs910-toolkit's `text_render`.
use crate::{font_atlas::Atlas, text_render::Vertex};
use wgpu::util::DeviceExt;

pub const SHADER: &str = r#"
struct VOut { @builtin(position) p:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) colour:vec4<f32> };
@vertex fn vs_main(@location(0) p:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) colour:vec4<f32>)->VOut {
    var o:VOut;o.p=vec4<f32>(p,0.,1.);o.uv=uv;o.colour=colour;return o;
}
@group(0) @binding(0) var tex:texture_2d<f32>;
@group(0) @binding(1) var smp:sampler;
@fragment fn fs_main(i:VOut)->@location(0) vec4<f32> {
    let c=textureSample(tex,smp,i.uv)*i.colour;
    if c.a<=0. {discard;} return c;
}
"#;

/// The masked variant: the mask is sampled at the fragment's framebuffer
/// position relative to `mask_rect` and multiplies the alpha.
pub const MASKED_SHADER: &str = r#"
struct VOut { @builtin(position) p:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) colour:vec4<f32> };
@vertex fn vs_main(@location(0) p:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) colour:vec4<f32>)->VOut {
    var o:VOut;o.p=vec4<f32>(p,0.,1.);o.uv=uv;o.colour=colour;return o;
}
@group(0) @binding(0) var tex:texture_2d<f32>;
@group(0) @binding(1) var smp:sampler;
@group(1) @binding(0) var mask_tex:texture_2d<f32>;
@group(1) @binding(1) var mask_smp:sampler;
struct MaskParams { rect:vec4<f32>, flags:vec4<f32> };
@group(1) @binding(2) var<uniform> mask:MaskParams;
@fragment fn fs_main(i:VOut)->@location(0) vec4<f32> {
    let t=textureSample(tex,smp,i.uv);
    var c=t*i.colour;
    if mask.flags.x>0.5 { c=vec4<f32>(t.rgb*i.colour.rgb,i.colour.a); }
    let muv=(i.p.xy-mask.rect.xy)/mask.rect.zw;
    if muv.x<0. || muv.y<0. || muv.x>1. || muv.y>1. {discard;}
    let m=textureSample(mask_tex,mask_smp,muv);
    let a=c.a*m.a;
    if a<=0. {discard;} return vec4<f32>(c.rgb,a);
}
"#;

/// Render-target images: the texture alpha is ignored (opaque backbuffer copy).
pub const OPAQUE_SHADER: &str = r#"
struct VOut { @builtin(position) p:vec4<f32>,@location(0) uv:vec2<f32>,@location(1) colour:vec4<f32> };
@vertex fn vs_main(@location(0) p:vec2<f32>,@location(1) uv:vec2<f32>,@location(2) colour:vec4<f32>)->VOut {
    var o:VOut;o.p=vec4<f32>(p,0.,1.);o.uv=uv;o.colour=colour;return o;
}
@group(0) @binding(0) var tex:texture_2d<f32>;
@group(0) @binding(1) var smp:sampler;
@fragment fn fs_main(i:VOut)->@location(0) vec4<f32> {
    let t=textureSample(tex,smp,i.uv);
    let c=vec4<f32>(t.rgb*i.colour.rgb,i.colour.a);
    if c.a<=0. {discard;} return c;
}
"#;

#[derive(Clone)]
pub struct Texture {
    pub bind: wgpu::BindGroup,
    _texture: Option<wgpu::Texture>,
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
}

pub struct Pipeline {
    pipeline: wgpu::RenderPipeline,
    /// The same batch shader with a second texture whose alpha masks the
    /// output by framebuffer position (a second texture unit modulates
    /// alpha).
    masked_pipeline: wgpu::RenderPipeline,
    /// Render-target images (the minimap base sprite) carry no meaningful
    /// alpha: the client copies the opaque backbuffer into the sprite, so the
    /// texture alpha is ignored and the vertex alpha rules.
    opaque_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    mask_layout: wgpu::BindGroupLayout,
}

#[derive(Clone)]
pub struct Mesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

/// A painter's quads for one frame in one vertex buffer, rewritten by
/// [`QuadBuffer::upload`] and grown by doubling, over an index buffer of the
/// same quad pattern as [`Pipeline::mesh`] (`4i, 4i+2, 4i+1, 4i+2, 4i+3,
/// 4i+1`), so a batch of quads `first..first + n` is the index range
/// `6 first..6 (first + n)`: the vertices each draw fetches are the ones its
/// own [`Mesh`] held (programme Phase 6: no buffer per batch per frame).
#[derive(Clone)]
pub struct QuadBuffer {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    capacity: u32,
}

impl QuadBuffer {
    /// Write `vertices` (whole quads) at the start of `slot`'s buffer,
    /// replacing it with one of twice the needed capacity when too small.
    pub fn upload(
        slot: &mut Option<Self>,
        device: &wgpu::Device,
        queue: &dyn crate::uploads::Uploader,
        vertices: &[Vertex],
    ) {
        let quads = (vertices.len() / 4) as u32;
        if slot.as_ref().is_none_or(|b| b.capacity < quads) {
            let capacity = quads.next_power_of_two().max(256);
            let indices: Vec<u32> = (0..capacity)
                .flat_map(|i| [i * 4, i * 4 + 2, i * 4 + 1, i * 4 + 2, i * 4 + 3, i * 4 + 1])
                .collect();
            *slot = Some(Self {
                vertices: device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("text vertices"),
                    size: u64::from(capacity) * 4 * std::mem::size_of::<Vertex>() as u64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("text quad indices"),
                    contents: bytemuck::cast_slice(&indices),
                    usage: wgpu::BufferUsages::INDEX,
                }),
                capacity,
            });
        }
        if !vertices.is_empty() {
            let buffer = slot.as_ref().unwrap();
            queue.write_buffer(&buffer.vertices, 0, bytemuck::cast_slice(vertices));
        }
    }
}

/// What a batch draws: its own [`Mesh`] or quads of a [`QuadBuffer`].
#[derive(Clone)]
pub enum Geometry<'a> {
    Mesh(&'a Mesh),
    Quads(&'a QuadBuffer, std::ops::Range<u32>),
}

impl<'a> From<&'a Mesh> for Geometry<'a> {
    fn from(mesh: &'a Mesh) -> Self {
        Self::Mesh(mesh)
    }
}

impl<'a> Geometry<'a> {
    fn empty(&self) -> bool {
        match self {
            Self::Mesh(mesh) => mesh.count == 0,
            Self::Quads(_, quads) => quads.is_empty(),
        }
    }
    /// Bind the vertex and index buffers and draw.
    fn draw(&self, pass: &mut wgpu::RenderPass<'a>) {
        match self {
            Self::Mesh(mesh) => {
                pass.set_vertex_buffer(0, mesh.vertices.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.count, 0, 0..1);
            }
            Self::Quads(buffer, quads) => {
                pass.set_vertex_buffer(0, buffer.vertices.slice(..));
                pass.set_index_buffer(buffer.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(quads.start * 6..quads.end * 6, 0, 0..1);
            }
        }
    }
}

impl Pipeline {
    /// The one 2D batch pipeline set for `format` in `cache`
    /// ([`crate::pipelines::Variant::Batch2d`]).
    pub fn cached(
        cache: &crate::pipelines::PipelineCache,
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
    ) -> std::sync::Arc<Self> {
        cache.get(1, format, crate::pipelines::Variant::Batch2d, || {
            Self::new(device, format)
        })
    }
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("text atlas"),
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
            ],
        });
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite mask"),
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
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("text"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let masked_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("masked sprite"),
            bind_group_layouts: &[Some(&layout), Some(&mask_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("BatchedSprite"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let masked_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("MaskedSprite"),
            source: wgpu::ShaderSource::Wgsl(MASKED_SHADER.into()),
        });
        let opaque_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("OpaqueSprite"),
            source: wgpu::ShaderSource::Wgsl(OPAQUE_SHADER.into()),
        });
        let attributes = wgpu::vertex_attr_array![0=>Float32x2,1=>Float32x2,2=>Unorm8x4];
        let make = |pl: &wgpu::PipelineLayout, shader: &wgpu::ShaderModule| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("batched text"),
                layout: Some(pl),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        // Standard alpha blend with the normal framebuffer
                        // alpha mode 0. No sRGB encode.
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
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make(&pl, &shader);
        let masked_pipeline = make(&masked_pl, &masked_shader);
        let opaque_pipeline = make(&pl, &opaque_shader);
        Self {
            pipeline,
            masked_pipeline,
            opaque_pipeline,
            layout,
            mask_layout,
        }
    }
    pub fn draw_opaque<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        texture: &'a Texture,
        geometry: Geometry<'a>,
    ) {
        pass.set_pipeline(&self.opaque_pipeline);
        pass.set_bind_group(0, &texture.bind, &[]);
        geometry.draw(pass);
    }
    /// Wrap an existing render target (the minimap base sprite) as a 2D image.
    pub fn wrap(&self, device: &wgpu::Device, view: wgpu::TextureView) -> Texture {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("wrapped 2D texture"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Texture {
            bind,
            _texture: None,
            view,
            sampler,
        }
    }
    /// The mask bind group: the mask texture, its framebuffer rectangle
    /// `[x, y, w, h]` (the component origin) and
    /// the opaque-source flag.
    pub fn mask_bind_group(
        &self,
        device: &wgpu::Device,
        mask: &Texture,
        rect: [f32; 4],
        opaque: bool,
    ) -> wgpu::BindGroup {
        use wgpu::util::DeviceExt as _;
        let params: [f32; 8] = [
            rect[0],
            rect[1],
            rect[2],
            rect[3],
            if opaque { 1.0 } else { 0.0 },
            0.0,
            0.0,
            0.0,
        ];
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mask params"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite mask"),
            layout: &self.mask_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&mask.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&mask.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
            ],
        })
    }
    pub fn draw_masked<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        texture: &'a Texture,
        mask: &'a wgpu::BindGroup,
        geometry: Geometry<'a>,
    ) {
        pass.set_pipeline(&self.masked_pipeline);
        pass.set_bind_group(0, &texture.bind, &[]);
        pass.set_bind_group(1, mask, &[]);
        geometry.draw(pass);
    }
    pub fn upload(&self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: &Atlas) -> Texture {
        let filter = if atlas.scale == 1 {
            wgpu::FilterMode::Nearest
        } else {
            wgpu::FilterMode::Linear
        };
        self.upload_argb(
            device,
            queue,
            [atlas.width as u32, atlas.height as u32],
            &atlas.argb,
            filter,
        )
    }
    /// Normal sprites use linear filtering and repeat wrapping. Fonts choose
    /// their own verified filtering above.
    pub fn upload_argb(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: [u32; 2],
        pixels: &[u32],
        filter: wgpu::FilterMode,
    ) -> Texture {
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("2D texture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let rgba: Vec<u8> = pixels
            .iter()
            .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8])
            .collect();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(extent.width * 4),
                rows_per_image: Some(extent.height),
            },
            extent,
        );
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("2D filtering"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: filter,
            min_filter: filter,
            ..Default::default()
        });
        let view = texture.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("2D texture"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Texture {
            bind,
            _texture: Some(texture),
            view,
            sampler,
        }
    }
    pub fn mesh(&self, device: &wgpu::Device, quads: &[[Vertex; 4]]) -> Mesh {
        let vertices: Vec<Vertex> = quads.iter().flatten().copied().collect();
        let indices: Vec<u32> = (0..quads.len() as u32)
            .flat_map(|i| [i * 4, i * 4 + 2, i * 4 + 1, i * 4 + 2, i * 4 + 3, i * 4 + 1])
            .collect();
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("text vertices"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("text indices"),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
        Mesh {
            vertices,
            indices: index_buffer,
            count: indices.len() as u32,
        }
    }
    /// Draw inside the caller's colour pass, in caller-provided painter order.
    pub fn draw<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        texture: &'a Texture,
        geometry: Geometry<'a>,
    ) {
        if geometry.empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &texture.bind, &[]);
        geometry.draw(pass);
    }
}
