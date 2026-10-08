//! GPU model billboards: camera-facing quads a model carries, drawn right
//! after the model's own batches with the particle shader's fogged and
//! unfogged variants.
//!
//! The placement math is toolkit-neutral and lives in `crate::billboard`;
//! this module owns the GPU half:
//!
//! - [`MeshBillboards`]: the billboards an uploaded model mesh carries
//!   (`FloorMesh::billboards`), with the model's draw transform and the
//!   alpha reference its last batch left behind.
//! - [`Frame::add`]: the model's frustum early-out, then per billboard:
//!   skip the bloom-hidden ones while bloom is on, place the unit quad in
//!   view space, take it back to the scene frame through the inverse camera
//!   and record the quad's colour and fog amount.
//! - [`BillboardPass`]: one quad per billboard in model order, placed right
//!   after the owning mesh in the scene lists; depth test on, depth writes
//!   only when the model has no transparency, blended (`SRC_ALPHA,
//!   ONE_MINUS_SRC_ALPHA`, alpha test `> 0`) or, when the model's last batch
//!   was alpha-tested, unblended with alpha test `> ref`.
//!
//! The fragment colour is `texture * quad colour`: the quad's own vertex
//! colour carries the billboard colour. The fogged variant mixes towards the
//! distance fog colour by the per-draw amount, which rides in a vertex
//! attribute equal on all four corners (0 = unfogged).

use std::collections::HashMap;
use std::ops::Range;

use wgpu::util::DeviceExt;

use crate::actor_matrix::Matrix;
#[cfg(test)]
use crate::billboard::BillboardInstance;
use crate::billboard::QUAD_CORNERS;
pub use rs910_model::mesh_billboards::*;

/// The six normalised frustum planes from the `view * projection` entries
/// (the `z`, `-z`, `x`, `-x`, `y` and `-y` clip bounds).
#[must_use]
pub fn frustum_planes(vp: &[f32; 16]) -> [[f32; 4]; 6] {
    let plane = |col: usize, sign: f32| -> [f32; 4] {
        let e = vp;
        let nx = e[3] + sign * e[col];
        let ny = e[7] + sign * e[4 + col];
        let nz = e[11] + sign * e[8 + col];
        let length = f64::from(nz * nz + nx * nx + ny * ny).sqrt();
        [
            (f64::from(nx) / length) as f32,
            (f64::from(ny) / length) as f32,
            (f64::from(nz) / length) as f32,
            (f64::from(e[15] + sign * e[12 + col]) / length) as f32,
        ]
    };
    [
        plane(2, 1.0),
        plane(2, -1.0),
        plane(0, 1.0),
        plane(0, -1.0),
        plane(1, 1.0),
        plane(1, -1.0),
    ]
}

/// The model draw's early-out: no unique vertices, or the bounding cylinder
/// (`(0, min_y..max_y, 0)` plus its radius) outside one frustum plane.
#[must_use]
pub fn culled(b: &MeshBillboards, world: &[f32; 16], planes: &[[f32; 4]; 6]) -> bool {
    if !b.drawn {
        return true;
    }
    let (min_y, max_y, radius) = b.bounds;
    // The cylinder axis point at height y: x' = e4 * y + e12 ...
    let point = |y: f32| -> [f32; 3] { std::array::from_fn(|i| world[4 + i] * y + world[12 + i]) };
    let [bottom_x, bottom_y, bottom_z] = point(min_y as f32);
    let [top_x, top_y, top_z] = point(max_y as f32);
    planes.iter().any(|p| {
        let bottom = p[2] * bottom_z + p[0] * bottom_x + p[1] * bottom_y + p[3] + radius as f32;
        let top = p[2] * top_z + p[0] * top_x + p[1] * top_y + p[3] + radius as f32;
        bottom < 0.0 && top < 0.0
    })
}

/// One quad corner (`VERTEX | COLOR | TEX_COORD_2` of the unit quad, plus
/// the draw's fog amount).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BillboardVertex {
    pub pos: [f32; 3],
    /// `DiffuseColour` R, G, B, A bytes.
    pub colour: [u8; 4],
    pub uv: [f32; 2],
    /// `DistanceFogAmount` (0 under `NoFog`).
    pub fog: f32,
}

/// One billboard quad draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Draw {
    /// Material id (`-1` = no texture).
    pub material: i32,
    pub alpha_ref: u8,
    pub depth_write: bool,
    pub first_vertex: u32,
}

/// The draws of one mesh, placed after mesh `at` - 1 of scene list `list`
/// (0 opaque, 1 transparent), or after interface model `at`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub list: u8,
    pub at: usize,
    pub draws: Range<usize>,
}

/// Every billboard quad of one frame.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub vertices: Vec<BillboardVertex>,
    pub draws: Vec<Draw>,
    pub segments: Vec<Segment>,
}

/// The camera inputs of one frame's billboards.
#[derive(Clone, Debug)]
pub struct View {
    /// The view entries in the frame the meshes' world matrices use.
    pub view: [f32; 16],
    /// The inverse camera.
    pub inverse: Matrix,
    /// The frustum planes.
    pub planes: [[f32; 4]; 6],
    /// Fog start and end, when fog is on.
    pub fog: Option<(f32, f32)>,
    /// Whether bloom is enabled.
    pub bloom: bool,
}

impl View {
    /// `view`/`projection` are 4x4 matrix entries.
    #[must_use]
    pub fn new(
        view: [f32; 16],
        projection: &[f32; 16],
        fog: Option<(f32, f32)>,
        bloom: bool,
    ) -> Self {
        let inverse = Matrix(std::array::from_fn(|i| view[i / 3 * 4 + i % 3])).inverse();
        let vp = crate::camera::multiply(&view, projection);
        Self {
            view,
            inverse,
            planes: frustum_planes(&vp),
            fog,
            bloom,
        }
    }
}

impl Frame {
    /// The billboard tail of one mesh's draw: `world` is the model's matrix
    /// in the view's frame.
    pub fn add(&mut self, list: u8, at: usize, b: &MeshBillboards, world: &[f32; 16], view: &View) {
        if culled(b, world, &view.planes) {
            return;
        }
        let mv = crate::camera::multiply(world, &view.view);
        let start = self.draws.len();
        for instance in &b.instances {
            if instance.face.bloom_hidden && view.bloom {
                continue;
            }
            let placement = crate::billboard::place(instance, &mv);
            let quad = Matrix(placement.quad);
            let fog = view.fog.map_or(0.0, |(s, e)| {
                crate::billboard::fog_amount(placement.view[2], s, e)
            });
            let c = instance.state.colour;
            let colour = [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8];
            let first_vertex = self.vertices.len() as u32;
            for [x, y] in QUAD_CORNERS {
                let v = quad.point(x, y, 0.0);
                self.vertices.push(BillboardVertex {
                    pos: view.inverse.point(v[0], v[1], v[2]),
                    colour,
                    uv: [x, y],
                    fog,
                });
            }
            self.draws.push(Draw {
                material: instance.face.material,
                alpha_ref: b.alpha_ref,
                depth_write: !b.has_transparency,
                first_vertex,
            });
        }
        if self.draws.len() > start {
            self.segments.push(Segment {
                list,
                at,
                draws: start..self.draws.len(),
            });
        }
    }
}

/// The billboard shader: `texture * quad colour`, then a mix towards the
/// distance fog colour (alpha kept) by the quad's fog amount.
pub const BILLBOARD_SHADER: &str = r#"
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
struct DrawUniforms {
    alpha: vec4<f32>,   // x: alpha reference (GL_GREATER)
};
@group(0) @binding(0) var<uniform> u: FloorUniforms;
@group(1) @binding(0) var diffuse_tex: texture_2d<f32>;
@group(1) @binding(1) var diffuse_sampler: sampler;
@group(1) @binding(2) var<uniform> b: DrawUniforms;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) colour: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) fog: f32,
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
    out.colour = v.colour;
    out.uv = v.uv;
    out.fog = v.fog;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var colour = textureSample(diffuse_tex, diffuse_sampler, in.uv) * in.colour;
    let fogged = vec4<f32>(u.distance_fog_colour.xyz, colour.w);
    colour = colour + in.fog * (fogged - colour);
    if (colour.w <= b.alpha.x) { discard; }
    return colour;
}
"#;

const VERTEX_ATTRS: [wgpu::VertexAttribute; 4] =
    wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4, 2 => Float32x2, 3 => Float32];

/// The billboard pass's pipelines by `(alpha tested, depth write)` and its
/// material layout, for one sample count and colour format
/// ([`crate::pipelines::Variant::Billboards`]).
pub struct BillboardPipelines {
    variants: [wgpu::RenderPipeline; 4],
    layout: wgpu::BindGroupLayout,
}

impl BillboardPipelines {
    fn get(&self, cutout: bool, depth_write: bool) -> &wgpu::RenderPipeline {
        &self.variants[usize::from(cutout) * 2 + usize::from(depth_write)]
    }
}

/// The scene-pass (and interface) billboard draw owner.
pub struct BillboardPass {
    pipelines: std::sync::Arc<BillboardPipelines>,
    index_buffer: wgpu::Buffer,
    vertex_buffer: Option<wgpu::Buffer>,
    vertex_capacity: usize,
    bindings: HashMap<(i32, u8), wgpu::BindGroup>,
    draws: Vec<Draw>,
    segments: Vec<Segment>,
}

impl BillboardPipelines {
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
            crate::pipelines::Variant::Billboards,
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
            label: Some("billboards"),
            source: wgpu::ShaderSource::Wgsl(std::borrow::Cow::Borrowed(BILLBOARD_SHADER)),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("billboard material"),
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
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("billboard layout"),
            bind_group_layouts: &[Some(floor.uniform_layout()), Some(&layout)],
            immediate_size: 0,
        });
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
                label: Some("billboards"),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<BillboardVertex>() as wgpu::BufferAddress,
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
                // Camera-facing quads: no culling, like the particle quads.
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(depth_write),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
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
}

impl BillboardPass {
    /// A pass (its quad indices and per-frame state) drawing with
    /// `pipelines` (`BillboardPipelines::cached`).
    pub fn new(device: &wgpu::Device, pipelines: std::sync::Arc<BillboardPipelines>) -> Self {
        // TRIANGLEFAN(0,1,2)(0,2,3) as a list.
        let index_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("billboard quad"),
            contents: bytemuck::cast_slice(&[0_u16, 1, 2, 0, 2, 3]),
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

    /// Upload this frame's quads and resolve each draw's material through
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
        self.clear();
        if frame.draws.is_empty() {
            return Ok(());
        }
        if self.vertex_capacity < frame.vertices.len() {
            self.vertex_capacity = frame.vertices.len().next_power_of_two();
            self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("billboard vertices"),
                size: (self.vertex_capacity * std::mem::size_of::<BillboardVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        queue.write_buffer(
            self.vertex_buffer.as_ref().expect("allocated above"),
            0,
            bytemuck::cast_slice(&frame.vertices),
        );
        for draw in &frame.draws {
            let key = (draw.material, draw.alpha_ref);
            if !self.bindings.contains_key(&key) {
                // Without a material the white texture stands in for an
                // unbound sampler, like the particle path.
                let material = (draw.material >= 0)
                    .then(|| materials.get(u32::from(draw.material as u16)))
                    .flatten();
                let texture = textures.texture_or_white(
                    device,
                    queue.queue(),
                    pack,
                    materials,
                    if material.is_some_and(|m| m.diffuse_texture.is_some()) {
                        draw.material
                    } else {
                        -1
                    },
                )?;
                let alpha = [f32::from(draw.alpha_ref) / 255.0, 0., 0., 0.];
                let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("billboard draw"),
                    contents: bytemuck::cast_slice(&alpha),
                    usage: wgpu::BufferUsages::UNIFORM,
                });
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("billboard material"),
                    layout: &self.pipelines.layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(texture.0),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::Sampler(texture.1),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: buffer.as_entire_binding(),
                        },
                    ],
                });
                self.bindings.insert(key, bind_group);
            }
        }
        self.draws.clone_from(&frame.draws);
        self.segments.clone_from(&frame.segments);
        Ok(())
    }

    /// No billboards this frame.
    pub fn clear(&mut self) {
        self.draws.clear();
        self.segments.clear();
    }

    /// Mesh positions of `list` that billboards follow, clamped to `len`.
    #[must_use]
    pub fn splits(&self, list: u8, len: usize) -> Vec<usize> {
        let mut splits: Vec<usize> = self
            .segments
            .iter()
            .filter(|s| s.list == list)
            .map(|s| s.at.min(len))
            .collect();
        splits.sort_unstable();
        splits.dedup();
        splits
    }

    /// Draw the segments of `list` placed at clamped mesh position `at`.
    pub fn draw_at<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        uniforms: &'a wgpu::BindGroup,
        list: u8,
        at: usize,
        len: usize,
    ) {
        for segment in &self.segments {
            if segment.list == list && segment.at.min(len) == at {
                self.draw_range(pass, uniforms, segment.draws.clone());
            }
        }
    }

    fn draw_range<'a>(
        &'a self,
        pass: &mut wgpu::RenderPass<'a>,
        uniforms: &'a wgpu::BindGroup,
        draws: Range<usize>,
    ) {
        let (Some(vertices), Some(draws)) = (self.vertex_buffer.as_ref(), self.draws.get(draws))
        else {
            return;
        };
        if draws.is_empty() {
            return;
        }
        pass.set_bind_group(0, uniforms, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), wgpu::IndexFormat::Uint16);
        for d in draws {
            pass.set_pipeline(self.pipelines.get(d.alpha_ref != 0, d.depth_write));
            pass.set_bind_group(1, &self.bindings[&(d.material, d.alpha_ref)], &[]);
            pass.draw_indexed(0..6, d.first_vertex as i32, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::billboard::{BillboardFace, BillboardState};

    fn instance(depth_offset: i32) -> BillboardInstance {
        BillboardInstance {
            centroid: [0.0, 0.0, 0.0],
            face: BillboardFace {
                face: 0,
                source_face: 0,
                vertices: [0, 1, 2],
                width: 10,
                height: 20,
                material: -1,
                bloom_hidden: false,
                depth_offset,
                sprite_mode: 1,
                sprite_blend: 2,
                remove_face: false,
            },
            state: BillboardState::new(0x80FF_4020_u32 as i32),
        }
    }

    fn identity() -> [f32; 16] {
        Matrix::default().entries()
    }

    fn billboards(instances: Vec<BillboardInstance>) -> MeshBillboards {
        MeshBillboards {
            instances,
            has_transparency: false,
            alpha_ref: 0,
            drawn: true,
            bounds: (-10, 10, 10),
            matrix: Matrix::default(),
            origin: [0.0; 3],
        }
    }

    /// A far orthographic box: nothing near the origin is culled.
    fn view() -> View {
        let mut projection = identity();
        projection[0] = 1.0 / 4096.0;
        projection[5] = 1.0 / 4096.0;
        projection[10] = 1.0 / 4096.0;
        View::new(identity(), &projection, None, false)
    }

    #[test]
    fn quad_corners_follow_the_fan_and_the_world_translation() {
        let mut frame = Frame::default();
        let mut world = identity();
        world[12] = 100.0;
        world[14] = 1000.0;
        frame.add(1, 3, &billboards(vec![instance(0)]), &world, &view());
        let corners: Vec<_> = frame.vertices.iter().map(|v| (v.pos, v.uv)).collect();
        assert_eq!(
            corners,
            vec![
                ([90.0, -20.0, 1000.0], [0.0, 0.0]),
                ([90.0, 20.0, 1000.0], [0.0, 1.0]),
                ([110.0, 20.0, 1000.0], [1.0, 1.0]),
                ([110.0, -20.0, 1000.0], [1.0, 0.0]),
            ]
        );
        assert_eq!(frame.vertices[0].colour, [0xFF, 0x40, 0x20, 0x80]);
        assert_eq!(
            frame.segments,
            vec![Segment {
                list: 1,
                at: 3,
                draws: 0..1
            }]
        );
        assert!(frame.draws[0].depth_write);
    }

    #[test]
    fn bloom_hides_flagged_billboards_and_culled_models_draw_none() {
        let mut hidden = instance(0);
        hidden.face.bloom_hidden = true;
        let mut v = view();
        v.bloom = true;
        let mut frame = Frame::default();
        frame.add(
            0,
            0,
            &billboards(vec![hidden, instance(0)]),
            &identity(),
            &v,
        );
        assert_eq!(frame.draws.len(), 1);
        let mut far = identity();
        far[12] = 1.0e6;
        let mut frame = Frame::default();
        frame.add(0, 0, &billboards(vec![instance(0)]), &far, &view());
        assert!(frame.draws.is_empty() && frame.segments.is_empty());
        let mut none = billboards(vec![instance(0)]);
        none.drawn = false;
        frame.add(0, 0, &none, &identity(), &view());
        assert!(frame.draws.is_empty());
    }

    #[test]
    fn frustum_planes_are_normalised_row_sums() {
        let p = frustum_planes(&identity());
        // First plane: (e3 + e2, e7 + e6, e11 + e10, e15 + e14) normalised.
        assert_eq!(p[0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(p[1], [0.0, 0.0, -1.0, 1.0]);
        assert_eq!(p[2], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(p[5], [0.0, -1.0, 0.0, 1.0]);
    }
}
