//! The water's GPU half (renderer plan M7 and Q-WATER2,
//! [`crate::water_body`]): the frame's water draws (the classic floor's
//! water batches, taken out of the floor draws), the per-level water mesh of the
//! floor's tile selection (`crate::water_body::water_mesh`, its beds), the
//! planar reflection pass, the scene copy the water looks through and the
//! water pass.
//!
//! Frame order with water (water draws in forward group 2, after group 0's
//! resolve): reflection (the frame's entity and floor draws under the mirrored
//! camera, the full viewport, one sample, over transparent black) → forward
//! group 0 (opaque entities, floors, alpha-tested billboards) → copy of the resolved HDR colour → water (opaque, depth
//! tested against the read-only scene depth, which it also samples) →
//! transparent entities and blended sprites. A frame without water draws
//! runs the M1 pass unchanged.
//!
//! Resources are built lazily from the forward pipeline's layouts
//! (`get_bind_group_layout`), so the renderer's constructor only holds the
//! empty state.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::frame::resources::FloorToken;
use crate::frame::*;
use crate::water_body::WaterMap;

/// The `Water` block of [`crate::water_body::WATER_WGSL`].
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct WaterUniforms {
    pub(crate) inv_view_proj: [[f32; 4]; 4],
    pub(crate) plane: [f32; 4],
    pub(crate) time: [f32; 4],
    pub(crate) size: [f32; 4],
    pub(crate) rect: [f32; 4],
    pub(crate) origin: [f32; 4],
    pub(crate) scales: [f32; 4],
    pub(crate) weights: [f32; 4],
    pub(crate) distort: [f32; 4],
    pub(crate) macro_weights: [f32; 4],
    pub(crate) macro_distort: [f32; 4],
    pub(crate) brdf: [f32; 4],
    pub(crate) reflect: [f32; 4],
    pub(crate) body: [f32; 4],
    pub(crate) extinction: [f32; 4],
    pub(crate) sky: [f32; 4],
    pub(crate) shore: [f32; 4],
    /// Lane Q-FX (`crate::water_body::effects::uniforms`).
    pub(crate) fx: [[f32; 4]; 2],
}

/// Reflection plane tolerance (fine units): a surface this far from the
/// plane still takes the planar image (classic water tiles are not exactly
/// level; farther ones take the sky gradient).
pub(crate) const PLANE_TOLERANCE: f32 = 96.0;
/// The oblique clip's slack above the plane (fine units).
pub(crate) const PLANE_BIAS: f32 = 4.0;

/// One water draw: a floor level's water batch.
#[derive(Clone, Copy, Debug)]
pub(crate) struct WaterDraw {
    pub(crate) level: usize,
    pub(crate) batch: usize,
    pub(crate) material: i32,
    pub(crate) instance: u32,
    pub(crate) count: u32,
}

/// A level's water: the per-vertex attributes (rebuilt with its floor) and
/// the water mesh of the floor's tile selection
/// ([`crate::water_body::water_mesh`], rebuilt when the selection changes).
pub(crate) struct LevelWater {
    pub(crate) token: FloorToken,
    /// Per floor vertex: depth, flow, type ([`crate::water_body::vertex_attributes`]).
    pub(crate) attrs: Vec<[f32; 4]>,
    /// The floor's vertex records ([`crate::models::mesh::floor_vertices`]).
    pub(crate) base: Vec<Vertex>,
    /// Per floor vertex: the bed ([`crate::water_body::beds`]).
    pub(crate) beds: Vec<Option<crate::water_body::Bed>>,
    /// Camera-independent (scene-local) water vertex positions, for the
    /// reflection plane.
    pub(crate) vertices: Vec<[f32; 3]>,
    pub(crate) selection: Option<crate::draw::FloorSelection>,
    pub(crate) mesh: Option<MeshGpu>,
}

/// A level's uploaded water mesh.
pub(crate) struct MeshGpu {
    pub(crate) vertices: wgpu::Buffer,
    pub(crate) colours: wgpu::Buffer,
    pub(crate) attrs: wgpu::Buffer,
    /// `(first, count)` by `FloorGpu::batches` index.
    pub(crate) ranges: HashMap<usize, (u32, u32)>,
}

pub(crate) struct Pipelines {
    pub(crate) water: wgpu::RenderPipeline,
    pub(crate) reflect: wgpu::RenderPipeline,
    pub(crate) reflect_no_depth_write: wgpu::RenderPipeline,
    pub(crate) group2: wgpu::BindGroupLayout,
    pub(crate) reflect_frame: wgpu::Buffer,
    pub(crate) reflect_frame_bind: wgpu::BindGroup,
    pub(crate) uniforms: wgpu::Buffer,
    pub(crate) water_sampler: wgpu::Sampler,
    pub(crate) clamp_sampler: wgpu::Sampler,
    pub(crate) flat_normal: wgpu::TextureView,
    /// The caustic rays' pipeline (`caustics`), when the module
    /// has them.
    pub(crate) caustics: Option<crate::frame::gpu::caustics::CausticPipes>,
}

pub(crate) struct WaterTargets {
    pub(crate) size: [u32; 2],
    pub(crate) reflection_size: [u32; 2],
    pub(crate) reflection_view: wgpu::TextureView,
    pub(crate) reflection_depth: wgpu::TextureView,
}

/// What the last frame's water did (diagnostics and tests).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WaterStats {
    pub draws: usize,
    /// The reflection plane (camera-local y) when the planar pass ran.
    pub plane: Option<f32>,
    pub water_type: u16,
    /// Map squares decoded for the scene and how many classic water tiles are
    /// modern water tiles (of all), level 0.
    pub squares: usize,
    pub agreement: (usize, usize),
    pub patches: usize,
    /// The model draws the planar reflection encoded, and those it left out
    /// as wholly outside its reflected frustum (`models::bounds`).
    pub reflection_draws: usize,
    pub reflection_culled: usize,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct WaterGpu {
    /// The pipelines per (sample count, debug view) (`frame::pipelines`).
    pub(crate) pipes: crate::frame::pipelines::Variants<(u32, u32), Pipelines>,
    /// The scene's water data under its key (base tile, level-0 floor
    /// identity); `None` inside: no usable map (no pack, or the map does not
    /// describe the floor, e.g. an instanced region).
    pub(crate) map: Option<((i32, i32, usize), Option<WaterMap>)>,
    pub(crate) types: Option<rs910_config::nxt::water_type::NxtWaterTypes>,
    pub(crate) levels: Vec<Option<LevelWater>>,
    pub(crate) normal_maps: HashMap<u16, wgpu::TextureView>,
    /// The land textures' mean colours by material (the bed colours).
    pub(crate) texture_means: HashMap<i32, [f32; 3]>,
    pub(crate) targets: Option<WaterTargets>,
    pub(crate) draws: Vec<WaterDraw>,
    /// The frame's draws the planar reflection encodes (indices into the
    /// frame's draw list, in its order): those whose box meets the
    /// reflected frustum, and every draw without a box.
    pub(crate) reflected: Vec<u32>,
    /// The first transparent-entity draw of the frame (the water goes
    /// before it).
    pub(crate) split: usize,
    pub(crate) bind: Option<wgpu::BindGroup>,
    pub(crate) normal_pair: [Option<u16>; 2],
    /// The frame's foam map material (WATERTYPE op 9) and the
    /// uploaded maps.
    pub(crate) foam: Option<u16>,
    pub(crate) foam_maps: HashMap<u16, wgpu::TextureView>,
    pub(crate) planar: bool,
    /// Log the next frame's statistics (a new scene).
    pub(crate) log_next: bool,
    pub(crate) stats: WaterStats,
    /// The frame's water plane (camera-local y; the reflection
    /// plane, also without the planar pass) and the caustics (`caustics`).
    pub(crate) plane_y: Option<f32>,
    pub(crate) caustics: crate::frame::gpu::caustics::CausticsGpu,
    /// Debug output (tests): 1 the fresnel with a flat normal, 2 the flow
    /// displacement.
    #[cfg(test)]
    pub(crate) debug: u32,
    /// Tests: the reflection draws every draw (the culling's reference).
    #[cfg(test)]
    pub(crate) test_no_reflection_cull: bool,
}

pub(crate) fn texture_2d(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

/// A normal map's texels with a box-filtered mip chain (`Rgba8Unorm`,
/// linear: the maps are tangent vectors, not colours).
pub(crate) fn upload_normal_map(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    img: &rs910_config::texture::RgbaImage,
) -> wgpu::TextureView {
    let levels = 32 - img.w.min(img.h).max(1).leading_zeros();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("modern water normal map"),
        size: wgpu::Extent3d {
            width: img.w,
            height: img.h,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (mut w, mut h) = (img.w, img.h);
    let mut px = img.px.clone();
    for level in 0..levels {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        if w == 1 || h == 1 {
            break;
        }
        let (nw, nh) = (w / 2, h / 2);
        let mut next = vec![0_u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let at = |xx: u32, yy: u32| u32::from(px[((yy * w + xx) * 4 + c) as usize]);
                    let sum = at(2 * x, 2 * y)
                        + at(2 * x + 1, 2 * y)
                        + at(2 * x, 2 * y + 1)
                        + at(2 * x + 1, 2 * y + 1);
                    next[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        px = next;
        (w, h) = (nw, nh);
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A material's diffuse texture's mean colour (linear light of the texels
/// under the classic texture gamma 0.7, as the forward pass shows them; white
/// without one): the bank colour's texture part (`crate::water_body::beds`).
pub(crate) fn texture_mean(snapshot: &SceneSnapshot<'_>, material: i32) -> [f32; 3] {
    let img = snapshot
        .pack
        .zip(snapshot.materials)
        .and_then(|(pack, mats)| {
            let texture = mats.get(u32::try_from(material).ok()?)?.diffuse_texture?;
            match rs910_config::texture::load_texture(pack, texture).ok()? {
                rs910_config::texture::Texture::Single(img) => Some(img),
                rs910_config::texture::Texture::Cube(_) => None,
            }
        });
    let Some(img) = img.filter(|i| !i.px.is_empty()) else {
        return [1.0; 3];
    };
    // Each texel value's linear light, once per value (the same numbers
    // as one `powf` per texel channel, performance plan P5).
    let linear: [f64; 256] = std::array::from_fn(|v| {
        (v as f64 / 255.0).powf(0.7 * f64::from(crate::post::tonemap::GAMMA))
    });
    let mut sum = [0.0_f64; 3];
    for p in img.px.chunks_exact(4) {
        for c in 0..3 {
            sum[c] += linear[usize::from(p[c])];
        }
    }
    let n = (img.px.len() / 4) as f64;
    sum.map(|v| (v / n) as f32)
}

impl Pipelines {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        forward: &wgpu::RenderPipeline,
        forward_module: &wgpu::ShaderModule,
        samples: u32,
        debug: u32,
        water_module: std::sync::Arc<wgpu::ShaderModule>,
    ) -> Self {
        let caustics = true;
        let group = |i| forward.get_bind_group_layout(i);
        let (g0, g1, g3, g4) = (group(0), group(1), group(3), group(4));
        let receive = group(2);
        let fragment_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty,
            count: None,
        };
        let float_texture = |filterable, dim| wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: dim,
            multisampled: false,
        };
        let uniform = wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        };
        // Group 2: the shadow receive bindings (0-2, as `shadow_receive_
        // layout`) and the water's (10-18).
        let group2 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern water group 2"),
            entries: &[
                fragment_entry(0, uniform),
                fragment_entry(
                    1,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                ),
                fragment_entry(
                    2,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: uniform,
                    count: None,
                },
                fragment_entry(11, float_texture(false, wgpu::TextureViewDimension::D2)),
                fragment_entry(
                    12,
                    wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: samples > 1,
                    },
                ),
                fragment_entry(13, float_texture(true, wgpu::TextureViewDimension::D2)),
                fragment_entry(14, float_texture(true, wgpu::TextureViewDimension::D2)),
                fragment_entry(15, float_texture(true, wgpu::TextureViewDimension::D2)),
                // The foam map (water_body::effects).
                fragment_entry(16, float_texture(true, wgpu::TextureViewDimension::D2)),
                fragment_entry(
                    17,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
                fragment_entry(
                    18,
                    wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                ),
            ],
        });
        let water_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern water"),
            bind_group_layouts: &[Some(&g0), Some(&g1), Some(&group2), Some(&g3), Some(&g4)],
            immediate_size: 0,
        });
        let forward_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern water reflection"),
            bind_group_layouts: &[Some(&g0), Some(&g1), Some(&receive), Some(&g3), Some(&g4)],
            immediate_size: 0,
        });
        let [v, c, i] = vertex_layouts();
        const ATTRS: [wgpu::VertexAttribute; 3] =
            wgpu::vertex_attr_array![12 => Float32x4, 13 => Float32x4, 14 => Float32x4];
        let water_attrs = wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &ATTRS,
        };
        let caustics = || {
            caustics.then(|| {
                crate::frame::gpu::caustics::CausticPipes::new(
                    device,
                    &water_module,
                    &g0,
                    &g1,
                    [v.clone(), c.clone(), i.clone(), Some(water_attrs.clone())],
                )
            })
        };
        let water = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern water"),
                layout: Some(&water_layout),
                vertex: wgpu::VertexState {
                    module: &water_module,
                    entry_point: Some("vs_water"),
                    buffers: &[v.clone(), c.clone(), i.clone(), Some(water_attrs.clone())],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &water_module,
                    entry_point: Some("fs_water"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    mask: !0,
                    alpha_to_coverage_enabled: false,
                },
                multiview_mask: None,
                cache: None,
            })
        };
        // The reflection pass: the forward pipelines with the winding
        // mirrored (the reflected view flips it), one sample.
        // Debug output 5 (verification): the reflection pass writes each
        // fragment's camera-local position instead of its colour.
        let positions = debug == crate::water_body::DEBUG_REFLECTION_POSITIONS;
        // Otherwise the forward pass's own entry points (the shading with
        // the probe ambient).
        let (reflect_module, reflect_vs, reflect_fs) = if positions {
            (&*water_module, "vs_main", "fs_reflect_position")
        } else {
            (forward_module, "vs_forward", "fs_forward")
        };
        let reflect_pipeline = |label: &str, write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&forward_layout),
                vertex: wgpu::VertexState {
                    module: reflect_module,
                    entry_point: Some(reflect_vs),
                    buffers: &[v.clone(), c.clone(), i.clone()],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: reflect_module,
                    entry_point: Some(reflect_fs),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: (!positions).then_some(ALPHA_BLEND),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Cw,
                    cull_mode: Some(wgpu::Face::Back),
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        // The caustic rays, the water surface and the two reflection
        // pipelines are created at once (`frame::compile`).
        let reflect_pipeline = &reflect_pipeline;
        let ((caustics, water), (reflect, reflect_no_depth_write)) = crate::frame::compile::join(
            || crate::frame::compile::join(caustics, water),
            || {
                crate::frame::compile::join(
                    || reflect_pipeline("modern water reflection", true),
                    || reflect_pipeline("modern water reflection (no depth write)", false),
                )
            },
        );
        let buffer = |label: &str, size: usize| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let reflect_frame = buffer(
            "modern water reflection frame",
            std::mem::size_of::<FrameUniforms>(),
        );
        // Group 0 carries the Frame uniforms and the AO map (M8); the
        // reflection pass binds a white AO map so SSAO never darkens it.
        let reflect_frame_bind = crate::frame::gpu::post::frame_bind(
            device,
            &g0,
            &reflect_frame,
            &crate::frame::gpu::post::white_view(device, queue),
        );
        let uniforms = buffer("modern water", std::mem::size_of::<WaterUniforms>());
        let sampler = |label: &str, mode| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some(label),
                address_mode_u: mode,
                address_mode_v: mode,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        let water_sampler = sampler("modern water normals", wgpu::AddressMode::Repeat);
        let clamp_sampler = sampler("modern water clamp", wgpu::AddressMode::ClampToEdge);
        let flat_normal = upload_normal_map(
            device,
            queue,
            &rs910_config::texture::RgbaImage {
                w: 1,
                h: 1,
                px: vec![128, 128, 255, 255],
            },
        );
        Self {
            water,
            reflect,
            reflect_no_depth_write,
            group2,
            reflect_frame,
            reflect_frame_bind,
            uniforms,
            water_sampler,
            clamp_sampler,
            flat_normal,
            caustics,
        }
    }
}

impl WaterGpu {
    /// Take a floor batch of `material` as a water draw (the frame's floor
    /// loop; `false`: it stays a floor draw).
    pub(crate) fn take(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        level: usize,
        batch: usize,
        material: i32,
        instance: u32,
        count: u32,
    ) -> bool {
        let Some(materials) = snapshot.materials else {
            return false;
        };
        if !crate::water_body::is_water_material(materials, material) {
            return false;
        }
        self.draws.push(WaterDraw {
            level,
            batch,
            material,
            instance,
            count,
        });
        true
    }

    /// The last frame's water (diagnostics, tests).
    pub(crate) fn stats(&self) -> WaterStats {
        self.stats
    }
}

/// A part of the forward pass with the water, each its own encode unit
/// (`frame::units`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WaterForward {
    /// Group 0 (opaque entities, floors, alpha-tested billboards), then the
    /// scene copy the water looks through.
    Group0,
    /// The water surfaces.
    Surface,
    /// Group 2 (transparent entities, blended sprites).
    Group2,
}

impl ModernRenderer {
    /// The water's pipelines at the current sample count and `debug` view
    /// (built the first time: at renderer creation, and on an
    /// anti-aliasing change). The depth comes from the drawn bed
    /// (`crate::water_body::BED_DEPTH_WGSL`), with the caustic rays.
    pub(crate) fn select_water_pipes(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        debug: u32,
    ) {
        let key = (self.samples, debug);
        if !self.water.pipes.contains(&key) {
            let pipes = self.water_pipes(device, queue, debug);
            self.water.pipes.insert_selected(key, pipes);
        }
        self.water.pipes.use_key(key);
    }

    /// The water's pipelines at the current sample count and `debug` view
    /// (a new set).
    pub(crate) fn water_pipes(
        &self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        debug: u32,
    ) -> Pipelines {
        let samples = self.samples;
        let forward = &self
            .pipelines
            .current()
            .expect("the forward-target pipelines")
            .forward;
        let module = self.shaders.get(
            device,
            crate::shaders::Module::Water {
                multisampled: samples > 1,
            },
        );
        Pipelines::new(
            device,
            queue,
            forward,
            &self.pipeline_inputs.forward_module,
            samples,
            debug,
            module,
        )
    }

    /// The last frame's water statistics (renderer plan M7).
    #[must_use]
    pub fn water_stats(&self) -> WaterStats {
        self.water.stats()
    }

    /// The scene's map-file water data, decoded once per installed scene.
    pub(crate) fn water_map(&mut self, snapshot: &SceneSnapshot<'_>) -> Option<&WaterMap> {
        let g0 = snapshot.floors.first().and_then(Option::as_ref)?;
        let id = g0.calls.as_ref().map_or(g0.stream0.as_ptr() as usize, |c| {
            std::sync::Arc::as_ptr(c) as usize
        });
        let key = (snapshot.floor_base[0], snapshot.floor_base[1], id);
        if self.water.map.as_ref().is_none_or(|(k, _)| *k != key) {
            let map = snapshot
                .pack
                .map(|pack| WaterMap::load(pack, snapshot.floor_base, [g0.tiles_x, g0.tiles_z]));
            self.water.map = Some((key, map));
            self.water.levels.clear();
            self.water.log_next = true;
        }
        self.water.map.as_ref().and_then(|(_, m)| m.as_ref())
    }

    /// Prepare this frame's water (after the draw records): the map, each
    /// level's water attributes and mesh, the normal maps, the
    /// targets, the reflection camera and the uniforms.
    pub(crate) fn prepare_water(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        rect: [i32; 4],
        size: [u32; 2],
        frame: &FrameUniforms,
    ) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        self.water.stats = WaterStats {
            draws: self.water.draws.len(),
            ..WaterStats::default()
        };
        self.water.bind = None;
        self.water.plane_y = None;
        if self.water.draws.is_empty() {
            return;
        }
        let debug = match crate::modern_debug_flags::flags().water {
            Some(crate::modern_debug_flags::WaterDebug::Term(n)) => n,
            _ => 0,
        };
        #[cfg(test)]
        let debug = if self.water.debug != 0 {
            self.water.debug
        } else {
            debug
        };
        self.select_water_pipes(device, queue, debug);
        if self.water.types.is_none() {
            self.water.types = Some(
                snapshot
                    .pack
                    .and_then(|p| rs910_config::nxt::water_type::NxtWaterTypes::load(p).ok())
                    .unwrap_or_default(),
            );
        }
        // The scene's map, taken out for the frame (lane Q-FIN: M7 cloned it
        // every frame) and put back below.
        self.water_map(snapshot);
        let map_entry = self.water.map.take();
        let map = map_entry.as_ref().and_then(|(_, m)| m.as_ref());
        // Each drawn level's water stream (rebuilt with its floor).
        let mut levels: Vec<usize> = self.water.draws.iter().map(|d| d.level).collect();
        levels.dedup();
        for level in levels {
            let Some(g) = snapshot.floors.get(level).and_then(Option::as_ref) else {
                continue;
            };
            if self.water.levels.len() <= level {
                self.water.levels.resize_with(level + 1, || None);
            }
            if self.water.levels[level]
                .as_ref()
                .is_some_and(|l| l.token.matches(g))
            {
                continue;
            }
            let water_batches: Vec<usize> = (0..g.batches.len())
                .filter(|&b| {
                    snapshot.materials.is_some_and(|m| {
                        crate::water_body::is_water_material(m, g.batches[b].material)
                    })
                })
                .collect();
            // The map describes this floor only where its water tiles are
            // NXT water tiles (not in an instanced copy of another area).
            let usable = map.filter(|m| {
                let (hits, total) = crate::water_body::agreement(g, &water_batches, m);
                if level == 0 {
                    self.water.stats.agreement = (hits, total);
                }
                total == 0 || hits * 10 >= total * 9
            });
            if level == 0 && map.is_some() && usable.is_none() {
                log::info!(
                    "[modern] water: the map files do not describe this floor ({:?} water tiles agree); default depth, no flow",
                    self.water.stats.agreement
                );
            }
            let attrs = crate::water_body::vertex_attributes(g, level, usable);
            let means = &mut self.water.texture_means;
            let is_water: Vec<bool> = (0..g.batches.len())
                .map(|b| water_batches.contains(&b))
                .collect();
            let stride = g.stride_floats;
            let mut used = vec![false; g.vertex_count];
            for &b in &water_batches {
                for (i, tris) in g.tile_tris.iter().enumerate() {
                    let owned = g.batches[b].tri_mask.get(i).copied().unwrap_or(0);
                    if let Some(tris) = tris {
                        for (t, tri) in tris.chunks_exact(3).enumerate() {
                            if owned & (1_u32 << (t % 32)) != 0 {
                                for &v in tri {
                                    used[usize::from(v)] = true;
                                }
                            }
                        }
                    }
                }
            }
            let vertices = (0..g.vertex_count)
                .filter(|&i| used[i])
                .map(|i| {
                    let f = &g.stream0[i * stride..i * stride + 3];
                    [f[0], f[1], f[2]]
                })
                .collect();
            self.water.levels[level] = Some(LevelWater {
                token: FloorToken::of(g),
                attrs,
                base: crate::models::mesh::floor_vertices(g),
                beds: crate::water_body::beds(g, &is_water, |m| {
                    *means.entry(m).or_insert_with(|| texture_mean(snapshot, m))
                }),
                vertices,
                selection: None,
                mesh: None,
            });
        }
        // Each drawn level's water mesh for its floor's tile selection.
        let mut levels: Vec<usize> = self.water.draws.iter().map(|d| d.level).collect();
        levels.dedup();
        for level in levels {
            let (Some(g), Some(Some(floor)), Some(Some(lw))) = (
                snapshot.floors.get(level).and_then(Option::as_ref),
                self.floors.get(level),
                self.water.levels.get_mut(level),
            ) else {
                continue;
            };
            let Some(selection) = floor.selection.as_ref() else {
                continue;
            };
            if lw.mesh.is_some() && lw.selection.as_ref() == Some(selection) {
                continue;
            }
            let tiles = selection.tiles(g.tiles_x, g.tiles_z);
            let batches: Vec<(usize, Vec<u16>)> = floor
                .batches
                .iter()
                .enumerate()
                .filter(|(_, b)| {
                    snapshot
                        .materials
                        .is_some_and(|m| crate::water_body::is_water_material(m, b.material))
                })
                .map(|(i, b)| (i, g.batches[b.source].build_indices(g, &tiles).0))
                .collect();
            let mesh = crate::water_body::water_mesh(&batches, &lw.attrs, &lw.beds);
            let mut vertices = Vec::with_capacity(mesh.sources.len().max(1));
            let mut colours = Vec::with_capacity(mesh.sources.len().max(1));
            for &(key, first, count) in &mesh.ranges {
                let source = &g.batches[floor.batches[key].source];
                for &v in &mesh.sources[first as usize..(first + count) as usize] {
                    vertices.push(lw.base.get(v as usize).copied().unwrap_or_default());
                    colours.push(source.colours.get(v as usize).map_or(0, |&c| c as u32));
                }
            }
            let mut attrs = mesh.attrs.clone();
            if vertices.is_empty() {
                vertices.push(Vertex::default());
                colours.push(0);
                attrs.push([0.0; 12]);
            }
            let buffer = |label: &str, contents: &[u8]| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some(label),
                    contents,
                    usage: wgpu::BufferUsages::VERTEX,
                })
            };
            lw.mesh = Some(MeshGpu {
                vertices: buffer("modern water vertices", bytemuck::cast_slice(&vertices)),
                colours: buffer("modern water colours", bytemuck::cast_slice(&colours)),
                attrs: buffer("modern water attributes", bytemuck::cast_slice(&attrs)),
                ranges: mesh.ranges.iter().map(|&(k, f, n)| (k, (f, n))).collect(),
            });
            lw.selection = Some(selection.clone());
        }
        if let Some(m) = map {
            self.water.stats.squares = m.squares;
            self.water.stats.patches = m.patches.len();
        }
        // The frame's water type: the one at the camera target.
        let target = [origin[0], origin[2]];
        let water_type = map.map_or(0, |m| m.flow_at(target[0], target[1]).1);
        self.water.map = map_entry;
        self.water.stats.water_type = water_type;
        let pair = self
            .water
            .types
            .as_ref()
            .and_then(|t| t.get(u32::from(water_type)))
            .map_or([None, None], |t| t.normal_materials());
        for material in pair.into_iter().flatten() {
            if self.water.normal_maps.contains_key(&material) {
                continue;
            }
            let img = snapshot
                .pack
                .zip(snapshot.materials)
                .and_then(|(pack, mats)| {
                    let texture = mats.get(u32::from(material))?.diffuse_texture?;
                    match rs910_config::texture::load_texture(pack, texture).ok()? {
                        rs910_config::texture::Texture::Single(img) => Some(img),
                        rs910_config::texture::Texture::Cube(_) => None,
                    }
                });
            if let Some(img) = img {
                let view = upload_normal_map(device, queue, &img);
                self.water.normal_maps.insert(material, view);
            } else {
                log::warn!("[modern] water: normal map material {material} did not load");
            }
        }
        self.water.normal_pair = pair;
        // The type's foam map (WATERTYPE op 9).
        self.water.foam = self
            .water
            .types
            .as_ref()
            .and_then(|t| t.get(u32::from(water_type)))
            .and_then(|t| t.foam_material);
        if let Some(material) = self.water.foam {
            if let std::collections::hash_map::Entry::Vacant(slot) =
                self.water.foam_maps.entry(material)
            {
                let img = snapshot
                    .pack
                    .zip(snapshot.materials)
                    .and_then(|(pack, mats)| {
                        let texture = mats.get(u32::from(material))?.diffuse_texture?;
                        match rs910_config::texture::load_texture(pack, texture).ok()? {
                            rs910_config::texture::Texture::Single(img) => Some(img),
                            rs910_config::texture::Texture::Cube(_) => None,
                        }
                    });
                if let Some(img) = img {
                    slot.insert(upload_normal_map(device, queue, &img));
                } else {
                    log::warn!("[modern] water: foam map material {material} did not load");
                    self.water.foam = None;
                }
            }
        }
        // No foam map: no foam.
        let fx = if self.water.foam.is_some() {
            crate::water_body::effects::FX_ALL
        } else {
            crate::water_body::effects::FX_ALL & !crate::water_body::effects::FX_FOAM
        };
        // Targets: the reflection at the viewport's size over the setting's
        // divisor (`ModernSettings::reflections`); the scene copy is the
        // frame's HDR twin.
        let [_, _, w, h] = rect;
        let d = self.settings.reflections.divisor().unwrap_or(1);
        let reflection_size = [(w.max(d) / d) as u32, (h.max(d) / d) as u32];
        if self
            .water
            .targets
            .as_ref()
            .is_none_or(|t| t.size != size || t.reflection_size != reflection_size)
        {
            // The scene copy (`RefractionRT`) is the frame's HDR twin
            // (`frame::passes::ALIASES`).
            let reflection = texture_2d(
                device,
                "modern water reflection",
                reflection_size,
                HDR_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            );
            let depth = texture_2d(
                device,
                "modern water reflection depth",
                reflection_size,
                DEPTH_FORMAT,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            self.water.targets = Some(WaterTargets {
                size,
                reflection_size,
                reflection_view: reflection.create_view(&Default::default()),
                reflection_depth: depth.create_view(&Default::default()),
            });
        }
        // The reflection plane: the water nearest the camera target.
        let drawn: Vec<usize> = self.water.draws.iter().map(|d| d.level).collect();
        let plane = crate::water_body::reflection_height(
            drawn
                .iter()
                .filter_map(|&l| self.water.levels.get(l).and_then(Option::as_ref))
                .flat_map(|l| l.vertices.iter())
                .map(|p| [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]]),
        );
        let view = glam::Mat4::from_cols_array_2d(&frame.view);
        let view_proj = glam::Mat4::from_cols_array_2d(&frame.view_proj);
        let env_only = crate::modern_debug_flags::flags().water
            == Some(crate::modern_debug_flags::WaterDebug::EnvOnly);
        self.water.planar =
            !env_only && plane.is_some() && self.settings.reflections.divisor().is_some();
        self.water.plane_y = plane;
        self.water.reflected.clear();
        if let (true, Some(h)) = (self.water.planar, plane) {
            self.water.stats.plane = Some(h);
            let (reflected_view, reflected_vp) =
                crate::water_body::reflection_view_proj(view, view_proj, h, PLANE_BIAS);
            self.cull_reflection(&reflected_vp);
            let mut reflected = *frame;
            reflected.view = reflected_view.to_cols_array_2d();
            reflected.view_proj = reflected_vp.to_cols_array_2d();
            reflected.eye[1] = 2.0 * h - frame.eye[1];
            // The reflection binds no occlusion map of its own
            // (a white texel), so it reads none.
            reflected.params[1] = 0.0;
            let pipes = self.water.pipes.current().expect("water pipelines");
            queue.write_buffer(&pipes.reflect_frame, 0, bytemuck::bytes_of(&reflected));
        }
        let look = crate::water_body::LOOK;
        let type_look = crate::water_body::TypeLook::of(
            self.water
                .types
                .as_ref()
                .and_then(|t| t.get(u32::from(water_type))),
        );
        let uniforms = WaterUniforms {
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            plane: [
                plane.unwrap_or(0.0),
                PLANE_TOLERANCE,
                if self.water.planar { 1.0 } else { 0.0 },
                debug as f32,
            ],
            time: [
                crate::water_body::water_seconds(self.frame_millis()),
                look.flow_speed,
                look.still_water_normal_strength,
                look.flow_noise_scale,
            ],
            size: [
                size[0] as f32,
                size[1] as f32,
                1.0 / size[0].max(1) as f32,
                1.0 / size[1].max(1) as f32,
            ],
            rect: [
                rect[0] as f32,
                rect[1] as f32,
                w.max(1) as f32,
                h.max(1) as f32,
            ],
            origin: [origin[0], origin[1], origin[2], 0.0],
            scales: [
                type_look.texture_scales[0],
                type_look.texture_scales[1],
                0.0,
                0.0,
            ],
            weights: [look.detail[0][0], look.detail[1][0], 0.0, 0.0],
            distort: [look.detail[0][1], look.detail[1][1], 0.0, 0.0],
            macro_weights: [look.macro_[0][0], look.macro_[1][0], 0.0, 0.0],
            macro_distort: [look.macro_[0][1], look.macro_[1][1], 0.0, 0.0],
            brdf: look.brdf,
            reflect: [look.reflection[0], look.reflection[1], look.distortion, 0.0],
            body: [
                type_look.tint[0],
                type_look.tint[1],
                type_look.tint[2],
                look.max_path,
            ],
            extinction: [
                look.extinction_depths[0],
                look.extinction_depths[1],
                look.extinction_depths[2],
                look.deep_brightness,
            ],
            // The sky through the modern composite.
            sky: {
                let e = self.look.sky_exposure();
                [self.clear[0] / e, self.clear[1] / e, self.clear[2] / e, 1.0]
            },
            shore: [
                look.bank_slope,
                look.visibility_share,
                // 1 = M10's terrain draws the bed this frame
                // (`crate::water_body::BED_DEPTH_WGSL`).
                if self.terrain.active { 1.0 } else { 0.0 },
                look.bed_brightness,
            ],
            fx: crate::water_body::effects::uniforms(fx),
        };
        let pipes = self.water.pipes.current().expect("water pipelines");
        queue.write_buffer(&pipes.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let targets = self.targets.as_ref().expect("targets");
        let wt = self.water.targets.as_ref().expect("water targets");
        let normal = |m: Option<u16>| {
            m.and_then(|m| self.water.normal_maps.get(&m))
                .unwrap_or(&pipes.flat_normal)
        };
        let [na, nb] = self.water.normal_pair;
        let tex = wgpu::BindingResource::TextureView;
        self.water.bind = Some(
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern water"),
                layout: &pipes.group2,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: self.shadow.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: tex(&self.shadow.atlas.1),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.shadow.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 10,
                        resource: pipes.uniforms.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 11,
                        resource: tex(&targets.scratch_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 12,
                        resource: tex(&targets.depth),
                    },
                    wgpu::BindGroupEntry {
                        binding: 13,
                        resource: tex(&wt.reflection_view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 14,
                        resource: tex(normal(na)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 15,
                        resource: tex(normal(nb.or(na))),
                    },
                    wgpu::BindGroupEntry {
                        binding: 16,
                        resource: tex(self
                            .water
                            .foam
                            .and_then(|m| self.water.foam_maps.get(&m))
                            .unwrap_or(&pipes.flat_normal)),
                    },
                    wgpu::BindGroupEntry {
                        binding: 17,
                        resource: wgpu::BindingResource::Sampler(&pipes.water_sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 18,
                        resource: wgpu::BindingResource::Sampler(&pipes.clamp_sampler),
                    },
                ],
            }),
        );
        // CLIENT910_MODERN_CHECK: every floor batch drawn once, as a floor or
        // as water.
        if crate::modern_debug_flags::flags().check {
            static MISMATCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let floors = self
                .draws
                .iter()
                .filter(|d| {
                    matches!(d.geometry, Geometry::Floor { level, .. }
                        if level < crate::frame::underwater::BED_LEVEL)
                })
                .count();
            let expected = self.stats.floor_batches;
            if self.water.log_next {
                log::info!(
                    "[modern] water check: {floors} floor + {} water draws for {expected} floor batches",
                    self.water.draws.len()
                );
            }
            // Each water draw's mesh range holds its batch's selected
            // triangles (the floor draw's index count).
            let short = self
                .water
                .draws
                .iter()
                .filter(|d| {
                    self.water
                        .levels
                        .get(d.level)
                        .and_then(Option::as_ref)
                        .and_then(|l| l.mesh.as_ref())
                        .and_then(|m| m.ranges.get(&d.batch))
                        .is_none_or(|&(_, n)| n != d.count)
                })
                .count();
            if short > 0 {
                let n = MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                log::warn!(
                    "[modern] water check: {short} water draws whose mesh range is not their selected triangles ({n} mismatching frames)"
                );
            }
            if floors + self.water.draws.len() != expected {
                let n = MISMATCHES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                log::warn!(
                    "[modern] water check: {floors} floor + {} water draws for {expected} floor batches ({n} mismatching frames)",
                    self.water.draws.len()
                );
            }
        }
        if self.water.log_next || self.frame.is_multiple_of(600) {
            self.water.log_next = false;
            log::info!("[modern] water: {:?}", self.water.stats);
        }
    }

    /// One water draw: its batch's range of the level's water mesh.
    pub(crate) fn draw_water<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, d: &WaterDraw) {
        let (Some(material), Some((instances, _))) =
            (self.textures.get(d.material), self.instance_buffer.as_ref())
        else {
            return;
        };
        let Some(mesh) = self
            .water
            .levels
            .get(d.level)
            .and_then(Option::as_ref)
            .and_then(|l| l.mesh.as_ref())
        else {
            return;
        };
        let Some(&(first, count)) = mesh.ranges.get(&d.batch) else {
            return;
        };
        pass.set_bind_group(1, &material.bind_group, &[]);
        pass.set_bind_group(4, &self.textures.arrays.bind, &[]);
        pass.set_vertex_buffer(0, mesh.vertices.slice(..));
        pass.set_vertex_buffer(1, mesh.colours.slice(..));
        pass.set_vertex_buffer(2, instances.slice(..));
        pass.set_vertex_buffer(3, mesh.attrs.slice(..));
        pass.draw(first..first + count, d.instance..d.instance + 1);
    }

    /// The water's planar reflection (pass type 1; its own encode unit,
    /// `frame::units`, before the forward pass that samples it).
    pub(crate) fn encode_water_reflection(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(pipes), Some(wt), Some(_)) = (
            self.water.pipes.current(),
            self.water.targets.as_ref(),
            self.water.bind.as_ref(),
        ) else {
            return;
        };
        let receive = &self.shadow.atlas.2;
        // Only with a reflection plane (otherwise the water reads no planar
        // image).
        if self.water.planar {
            // Transparent black: the water shader puts the sky behind the
            // reflected scene (premultiplied by the `ALPHA_BLEND` draws).
            let clear = wgpu::Color::TRANSPARENT;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::WaterReflection)),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &wt.reflection_view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(clear),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &wt.reflection_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &pipes.reflect_frame_bind, &[]);
            pass.set_bind_group(2, receive, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            self.encode_terrain(&mut pass, crate::frame::TerrainPass::Reflection);
            let mut current = None;
            let mut bound = crate::frame::submit::Bound::default();
            let mut cursor = 0;
            while cursor < self.water.reflected.len() {
                let i = self.water.reflected[cursor];
                let d = &self.draws[i as usize];
                if current != Some(d.pass) {
                    pass.set_pipeline(match d.pass {
                        Pass::Opaque => &pipes.reflect,
                        Pass::NoDepthWrite => &pipes.reflect_no_depth_write,
                    });
                    current = Some(d.pass);
                }
                let available = crate::frame::submit::far_run_len(&self.draws[i as usize..]);
                let contiguous = self.water.reflected[cursor..]
                    .iter()
                    .take(available)
                    .enumerate()
                    .take_while(|(offset, index)| **index == i + *offset as u32)
                    .count();
                cursor += self.submit_run(
                    &mut pass,
                    &self.draws[i as usize..i as usize + contiguous],
                    &mut bound,
                );
            }
        }
    }

    /// `part` of the forward pass with the water (see the module docs;
    /// each part is an encode unit of its own, `frame::units`); the water's
    /// split is the first transparent-entity draw (the reflection is
    /// [`Self::encode_water_reflection`]).
    pub(crate) fn encode_forward_with_water(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        cx: &EncodeFrame<'_>,
        part: WaterForward,
    ) {
        let targets = cx.targets;
        let colour = cx.colour;
        let receive = &self.shadow.atlas.2;
        let split = self.water.split;
        let set_view = |pass: &mut wgpu::RenderPass<'_>| cx.set_view(pass);
        let (Some(pipes), Some(_), Some(bind)) = (
            self.water.pipes.current(),
            self.water.targets.as_ref(),
            self.water.bind.as_ref(),
        ) else {
            return;
        };
        let depth_attachment = |load| {
            Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            })
        };
        // Resolves (lane P4-GPU: once at the boundary that reads them):
        // group 0 into the scene copy the water looks through, group 2 into
        // the frame's resolve; the water pass itself resolves nothing (group
        // 2 resolves the whole target after it).
        fn attachment<'a>(
            colour: &'a wgpu::TextureView,
            resolve: Option<&'a wgpu::TextureView>,
        ) -> Option<wgpu::RenderPassColorAttachment<'a>> {
            Some(wgpu::RenderPassColorAttachment {
                view: colour,
                resolve_target: resolve,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })
        }
        let msaa = targets.msaa.is_some();
        // Group 0: opaque entities, floors, alpha-tested billboards.
        let mut sprites = self.sprites.draws.iter().peekable();
        if part != WaterForward::Group0 {
            // The sprites group 0 draws (below): those placed before its
            // last draw, then its tail.
            while sprites.next_if(|s| s.after < split).is_some() {}
            while sprites
                .next_if(|s| {
                    s.after <= split
                        && s.pipeline == crate::frame::gpu::sprites::SpritePipeline::Cutout
                })
                .is_some()
            {}
        }
        if part == WaterForward::Group0 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::ForwardBeforeWater)),
                color_attachments: &[attachment(colour, msaa.then_some(&targets.scratch_view))],
                depth_stencil_attachment: depth_attachment(wgpu::LoadOp::Load),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            set_view(&mut pass);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(2, receive, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            self.encode_terrain(&mut pass, crate::frame::TerrainPass::Forward);
            let mut current = None;
            let mut bound = crate::frame::submit::Bound::default();
            let mut i = 0;
            while i < split {
                let d = &self.draws[i];
                while let Some(sprite) = sprites.next_if(|s| s.after <= i) {
                    self.sprites
                        .draw(&mut pass, &self.pipes().sprites, &self.textures, sprite);
                    current = None;
                    bound.reset();
                }
                if current != Some(d.pass) {
                    pass.set_pipeline(match d.pass {
                        Pass::Opaque => &self.pipes().forward,
                        Pass::NoDepthWrite => &self.pipes().forward_no_depth_write,
                    });
                    current = Some(d.pass);
                }
                let end = sprites.peek().map_or(split, |s| s.after).min(split);
                i += self.submit_run(&mut pass, &self.draws[i..end], &mut bound);
            }
            // Group 0's tail: the alpha-tested billboards placed after it.
            while let Some(sprite) = sprites.next_if(|s| {
                s.after <= split && s.pipeline == crate::frame::gpu::sprites::SpritePipeline::Cutout
            }) {
                self.sprites
                    .draw(&mut pass, &self.pipes().sprites, &self.textures, sprite);
            }
            drop(pass);
            // The scene copy the water looks through: at one sample, a
            // copy of the scissor (the water reads the copy only where the
            // scene depth shows geometry, inside it); multisampled, group
            // 0's resolve (above).
            let [l, t, r, b] = self.post.clip;
            if !msaa && r > l && b > t {
                let at = wgpu::Origin3d { x: l, y: t, z: 0 };
                encoder.copy_texture_to_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &targets.resolved,
                        mip_level: 0,
                        origin: at,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::TexelCopyTextureInfo {
                        texture: &targets.scratch,
                        mip_level: 0,
                        origin: at,
                        aspect: wgpu::TextureAspect::All,
                    },
                    wgpu::Extent3d {
                        width: r - l,
                        height: b - t,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        // Water: depth tested against the read-only scene depth, which the
        // shader also reads.
        if part == WaterForward::Surface {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::Water)),
                color_attachments: &[attachment(colour, None)],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.depth,
                    depth_ops: None,
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            set_view(&mut pass);
            pass.set_pipeline(&pipes.water);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(2, bind, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            for d in &self.water.draws {
                self.draw_water(&mut pass, d);
            }
        }
        // Group 2: the transparent entities and the blended sprites.
        if part == WaterForward::Group2 {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(self.begin_pass(crate::frame::passes::Pass::ForwardAfterWater)),
                color_attachments: &[attachment(colour, msaa.then_some(&targets.resolved_view))],
                depth_stencil_attachment: depth_attachment(wgpu::LoadOp::Load),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            set_view(&mut pass);
            pass.set_bind_group(0, &self.frame_bind, &[]);
            pass.set_bind_group(2, receive, &[]);
            pass.set_bind_group(3, &self.lights.bind, &[]);
            let mut current = None;
            let mut bound = crate::frame::submit::Bound::default();
            let mut i = split;
            while i < self.draws.len() {
                let d = &self.draws[i];
                while let Some(sprite) = sprites.next_if(|s| s.after <= i) {
                    self.sprites
                        .draw(&mut pass, &self.pipes().sprites, &self.textures, sprite);
                    current = None;
                    bound.reset();
                }
                if current != Some(d.pass) {
                    pass.set_pipeline(match d.pass {
                        Pass::Opaque => &self.pipes().forward,
                        Pass::NoDepthWrite => &self.pipes().forward_no_depth_write,
                    });
                    current = Some(d.pass);
                }
                let end = sprites
                    .peek()
                    .map_or(self.draws.len(), |s| s.after)
                    .min(self.draws.len());
                i += self.submit_run(&mut pass, &self.draws[i..end], &mut bound);
            }
            for sprite in sprites {
                self.sprites
                    .draw(&mut pass, &self.pipes().sprites, &self.textures, sprite);
            }
        }
    }
}
