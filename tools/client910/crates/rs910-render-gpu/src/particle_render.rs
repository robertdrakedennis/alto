//! The particle renderer: particle lists become camera-facing quads, which
//! the particle shader draws in the ordinary scene pass.
//!
//! The CPU half buckets particles by view depth (1600 buckets of 64 plus 64
//! overflow lists of 768, far buckets first), splits batches on material/lit
//! changes, and builds the camera-facing (optionally rotated) quads in the
//! client's corner and UV order.
//! The GPU half draws those batches in the ordinary scene pass through the
//! floor pipeline's group-0 camera/fog uniforms: depth test `LEQUAL` with
//! depth writes off, `SRC_ALPHA, ONE_MINUS_SRC_ALPHA` with the alpha test
//! `> 0`, or the replace path with the material threshold for alpha-tested
//! materials.
//!
//! Positions are target-relative (the camera target is the frame origin),
//! as every other scene draw in `render.rs`.

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::particle::DrawParticle;

/// Depth buckets.
const BUCKETS: usize = 1600;
/// Particles per bucket before the overflow lists.
const BUCKET_SIZE: usize = 64;
/// Overflow lists and their capacity.
const OVERFLOW_LISTS: usize = 64;
const OVERFLOW_SIZE: usize = 768;
/// The index buffer covers 8191 quads.
pub const MAX_BATCH_QUADS: usize = 8191;

/// One stream-0 vertex (`VERTEX | COLOR | TEX_COORD_2`, stride 24).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ParticleVertex {
    pub pos: [f32; 3],
    /// R, G, B, A bytes (the GL order).
    pub colour: [u8; 4],
    pub uv: [f32; 2],
}

/// One batch draw: consecutive particles sharing material and lit flag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Batch {
    /// Material id (`-1` untextured).
    pub texture: i32,
    /// Whether the batch is sun-lit (it only splits batches).
    pub lit: bool,
    pub first_vertex: u32,
    pub quads: u32,
}

/// One particle owner's draw placed in the scene's painter order: the
/// owner's batches draw right after the owner entity's models (scenery,
/// players, NPCs, spot animations and projectiles alike) inside the opaque
/// (`list` 0) or transparent (`list` 1) pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub list: u8,
    /// Position of the owner entity in the frame plan's list.
    pub entity: usize,
    /// Scene meshes of that list drawn before the particles (resolved from
    /// `entity` by [`Segment::resolve`]; `usize::MAX` = after the list).
    pub mesh_end: usize,
    /// `Frame::batches` of this owner.
    pub batches: std::ops::Range<usize>,
}

impl Segment {
    /// Map the owner's plan position to the cumulative scene-mesh count of
    /// its list (`ends[list][entity]`, meshes up to and including it).
    pub fn resolve(&mut self, ends: [&[usize]; 2]) {
        self.mesh_end = ends[usize::from(self.list.min(1))]
            .get(self.entity)
            .copied()
            .unwrap_or(usize::MAX);
    }
}

/// Every particle list of one frame, ready for upload.
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub vertices: Vec<ParticleVertex>,
    pub batches: Vec<Batch>,
    /// Particles dropped by the bucket capacities.
    pub dropped: usize,
    /// The owners' draw positions; empty means every batch draws after the
    /// transparent list.
    pub segments: Vec<Segment>,
}

/// The float cos/sin tables over 16384 steps.
fn trig_matrix(angle: i32) -> (f32, f32) {
    let a = f64::from(angle & 0x3FFF) * crate::trig::STEP;
    (a.cos() as f32, a.sin() as f32)
}

struct Buckets {
    counts: Vec<usize>,
    slots: Vec<[usize; BUCKET_SIZE]>,
    overflow_counts: [usize; OVERFLOW_LISTS],
    overflow: Vec<Vec<usize>>,
    used_overflow: usize,
}

impl Buckets {
    fn new() -> Self {
        Self {
            counts: vec![0; BUCKETS],
            slots: vec![[0; BUCKET_SIZE]; BUCKETS],
            overflow_counts: [0; OVERFLOW_LISTS],
            overflow: (0..OVERFLOW_LISTS)
                .map(|_| Vec::with_capacity(OVERFLOW_SIZE))
                .collect(),
            used_overflow: 0,
        }
    }

    fn clear(&mut self, range: usize) {
        self.used_overflow = 0;
        for c in &mut self.counts[..range.min(BUCKETS)] {
            *c = 0;
        }
        self.overflow_counts = [0; OVERFLOW_LISTS];
        for o in &mut self.overflow {
            o.clear();
        }
    }

    /// Add a particle to its bucket or overflow list. False when dropped.
    fn push(&mut self, bucket: usize, particle: usize) -> bool {
        if bucket >= BUCKETS {
            return false;
        }
        if self.counts[bucket] < BUCKET_SIZE {
            self.slots[bucket][self.counts[bucket]] = particle;
            self.counts[bucket] += 1;
            return true;
        }
        if self.counts[bucket] == BUCKET_SIZE {
            if self.used_overflow == OVERFLOW_LISTS {
                return false;
            }
            self.counts[bucket] += self.used_overflow + 1;
            self.used_overflow += 1;
        }
        let list = self.counts[bucket] - BUCKET_SIZE - 1;
        // The original client would overrun the 768-entry array here.
        if self.overflow_counts[list] == OVERFLOW_SIZE {
            return false;
        }
        self.overflow[list].push(particle);
        self.overflow_counts[list] += 1;
        true
    }
}

/// One material/lit run of a particle list whose particles are already in
/// the buckets, ready to emit quads.
struct Run<'a> {
    list: &'a [DrawParticle],
    /// Buckets in use (view-depth range after shifting).
    range: usize,
    texture: i32,
    lit: bool,
    view: &'a [f32; 16],
    /// Moves a particle's scene-local position into the view's frame.
    local: &'a dyn Fn(&DrawParticle) -> [f32; 3],
}

/// Builds [`Frame`]s from particle lists.
pub struct Builder {
    buckets: Buckets,
    depths: Vec<i32>,
}

impl Default for Builder {
    fn default() -> Self {
        Self {
            buckets: Buckets::new(),
            depths: Vec::new(),
        }
    }
}

impl Builder {
    /// Build one frame from every scene system's list when no frame plan
    /// places them (offline scenes): the lists are ordered by their farthest
    /// particle and drawn after the transparent list.
    pub fn build(
        &mut self,
        lists: &[&[DrawParticle]],
        view: &[f32; 16],
        offset: [i32; 3],
    ) -> Frame {
        let depth = |p: &DrawParticle| {
            let [x, y, z]: [f32; 3] =
                std::array::from_fn(|i| ((p.pos[i] >> 12) + offset[i]) as f32);
            z * view[10] + y * view[6] + x * view[2] + view[14]
        };
        let mut order: Vec<(f32, usize)> = lists
            .iter()
            .enumerate()
            .map(|(i, l)| (l.iter().map(depth).fold(f32::MIN, f32::max), i))
            .collect();
        order.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
        let mut frame = Frame::default();
        for (_, i) in order {
            self.add_list(&mut frame, lists[i], view, offset);
        }
        frame
    }

    /// One owner's `drawParticles` at plan position `entity` of `list`.
    pub fn add_segment(
        &mut self,
        frame: &mut Frame,
        list: u8,
        entity: usize,
        particles: &[DrawParticle],
        view: &[f32; 16],
        offset: [i32; 3],
    ) {
        let start = frame.batches.len();
        self.add_list(frame, particles, view, offset);
        if frame.batches.len() > start {
            frame.segments.push(Segment {
                list,
                entity,
                mesh_end: usize::MAX,
                batches: start..frame.batches.len(),
            });
        }
    }

    /// Add one particle list: `view` is the camera's view matrix with the
    /// target as origin and `offset` moves a scene-local fine position into
    /// that frame (`base * 512 - target`).
    pub fn add_list(
        &mut self,
        frame: &mut Frame,
        list: &[DrawParticle],
        view: &[f32; 16],
        offset: [i32; 3],
    ) {
        if list.is_empty() {
            return;
        }
        let local = |p: &DrawParticle| -> [f32; 3] {
            std::array::from_fn(|i| ((p.pos[i] >> 12) + offset[i]) as f32)
        };
        self.depths.clear();
        let mut min = i32::MAX;
        let mut max = 0;
        for p in list {
            let [x, y, z] = local(p);
            let d = (z * view[10] + y * view[6] + x * view[2] + view[14]) as i32;
            max = max.max(d);
            min = min.min(d);
            self.depths.push(d);
        }
        let mut range = max - min;
        let shift = if range + 2 > BUCKETS as i32 {
            let s = ilog(range) + 1 - ilog(BUCKETS as i32);
            range = (range >> s) + 2;
            s
        } else {
            range += 2;
            0
        };
        let range = range as usize;
        // Split into material/lit runs, each bucketed and drawn.
        let mut start = 0;
        while start < list.len() {
            self.buckets.clear(range);
            let texture = list[start].texture;
            let lit = list[start].lit;
            let mut end = start;
            while end < list.len() {
                let p = &list[end];
                if end > 0 && end > start && (p.texture != texture || p.lit != lit) {
                    break;
                }
                let bucket = (self.depths[end] - min) >> shift;
                if !self.buckets.push(bucket.max(0) as usize, end) {
                    frame.dropped += 1;
                }
                end += 1;
            }
            self.emit(
                frame,
                &Run {
                    list,
                    range,
                    texture,
                    lit,
                    view,
                    local: &local,
                },
            );
            start = end;
        }
    }

    /// Emit one run's quads: far buckets first, each bucket newest-first,
    /// then its overflow list newest-first.
    fn emit(&self, frame: &mut Frame, run: &Run<'_>) {
        let Run {
            list,
            range,
            texture,
            lit,
            view,
            local,
        } = *run;
        let right = [view[0], view[4], view[8]];
        let up = [view[1], view[5], view[9]];
        let first_vertex = frame.vertices.len() as u32;
        let mut quads = 0_usize;
        let mut put = |p: &DrawParticle, frame: &mut Frame| {
            if quads == MAX_BATCH_QUADS {
                frame.dropped += 1;
                return;
            }
            let c = p.colour;
            let colour = [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8];
            let pos = local(p);
            let s = (p.size >> 12) as f32;
            let (r, u) = if p.angle == 0 {
                (right.map(|v| v * s), up.map(|v| v * s))
            } else {
                // rotZ(angle) * scale(s) * inverse view rotation.
                let (cos, sin) = trig_matrix(i32::from(p.angle));
                (
                    std::array::from_fn(|i| s * cos * right[i] + s * sin * up[i]),
                    std::array::from_fn(|i| s * -sin * right[i] + s * cos * up[i]),
                )
            };
            let corner = |a: f32, b: f32| -> [f32; 3] {
                std::array::from_fn(|i| pos[i] + a * r[i] + b * u[i])
            };
            for (position, uv) in [
                (corner(-1., -1.), [0., 0.]),
                (corner(-1., 1.), [0., 1.]),
                (corner(1., 1.), [1., 1.]),
                (corner(1., -1.), [1., 0.]),
            ] {
                frame.vertices.push(ParticleVertex {
                    pos: position,
                    colour,
                    uv,
                });
            }
            quads += 1;
        };
        for bucket in (0..range.min(BUCKETS)).rev() {
            let count = self.buckets.counts[bucket];
            for i in (0..count.min(BUCKET_SIZE)).rev() {
                put(&list[self.buckets.slots[bucket][i]], frame);
            }
            if count > BUCKET_SIZE {
                let overflow = &self.buckets.overflow[count - BUCKET_SIZE - 1];
                for &i in overflow.iter().rev() {
                    put(&list[i], frame);
                }
            }
        }
        if quads > 0 {
            frame.batches.push(Batch {
                texture,
                lit,
                first_vertex,
                quads: quads as u32,
            });
        }
    }
}

/// The bit length.
fn ilog(v: i32) -> i32 {
    32 - (v as u32).leading_zeros() as i32
}

/// The particle shader: `texture * vertex colour`, then distance fog from
/// `saturate(dot(vertex, distance_fog_plane))`; a disabled fog has a zero
/// plane, which draws unfogged. The shader has no lighting inputs, so the
/// `lit` batch flag only splits batches. The client's fixed-function
/// fallback (used when a programmable shader fails to build) is not needed:
/// wgpu has no such failure path.
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
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("particle material"),
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
            label: Some("particle layout"),
            bind_group_layouts: &[Some(floor.uniform_layout()), Some(&layout)],
            immediate_size: 0,
        });
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
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: depth_format,
                    depth_write_enabled: Some(false),
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
                mesh_end: usize::MAX,
                batches: 0..frame.batches.len(),
            }]
        } else {
            frame.segments.clone()
        };
        if self.vertex_capacity < frame.vertices.len() {
            self.vertex_capacity = frame.vertices.len().next_power_of_two();
            self.vertex_buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("particle vertices"),
                size: (self.vertex_capacity * std::mem::size_of::<ParticleVertex>()) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
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
                let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("particle material"),
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

    /// Resolve every segment's scene-mesh position (see [`Segment::resolve`]).
    pub fn resolve_segments(&mut self, ends: [&[usize]; 2]) {
        for segment in &mut self.segments {
            if segment.entity != usize::MAX {
                segment.resolve(ends);
            }
        }
    }

    /// The sorted, distinct mesh positions of `list` that particles follow,
    /// clamped to the list length (a trailing split draws after the list).
    #[must_use]
    pub fn splits(&self, list: u8, len: usize) -> Vec<usize> {
        let mut splits: Vec<usize> = self
            .segments
            .iter()
            .filter(|s| s.list == list)
            .map(|s| s.mesh_end.min(len))
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
            if segment.list == list && segment.mesh_end.min(len) == at {
                self.draw_batches(pass, uniforms, segment.batches.clone());
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn particle(x: i32, z: i32, texture: i32) -> DrawParticle {
        DrawParticle {
            pos: [x << 12, 0, z << 12],
            colour: 0x80FF_4020_u32 as i32,
            size: 8 << 12,
            angle: 0,
            texture,
            lit: true,
        }
    }

    /// A view looking down +Z (identity rotation): depth is z.
    fn view() -> [f32; 16] {
        let mut v = [0.0; 16];
        v[0] = 1.0;
        v[5] = 1.0;
        v[10] = 1.0;
        v[15] = 1.0;
        v
    }

    #[test]
    fn quads_follow_the_corner_order_and_colour_bytes() {
        let mut frame = Frame::default();
        let mut b = Builder::default();
        b.add_list(&mut frame, &[particle(10, 20, -1)], &view(), [0; 3]);
        assert_eq!(
            frame.batches,
            vec![Batch {
                texture: -1,
                lit: true,
                first_vertex: 0,
                quads: 1
            }]
        );
        let v: Vec<_> = frame.vertices.iter().map(|v| (v.pos, v.uv)).collect();
        assert_eq!(
            v,
            vec![
                ([2., -8., 20.], [0., 0.]),
                ([2., 8., 20.], [0., 1.]),
                ([18., 8., 20.], [1., 1.]),
                ([18., -8., 20.], [1., 0.]),
            ]
        );
        assert_eq!(frame.vertices[0].colour, [0xFF, 0x40, 0x20, 0x80]);
    }

    #[test]
    fn batches_sort_far_first_and_split_on_material() {
        let mut frame = Frame::default();
        let mut b = Builder::default();
        let list = [
            particle(0, 5, 3),
            particle(0, 900, 3),
            particle(0, 50, 3),
            particle(0, 7, 4),
        ];
        b.add_list(&mut frame, &list, &view(), [0; 3]);
        assert_eq!(frame.batches.len(), 2);
        assert_eq!((frame.batches[0].texture, frame.batches[0].quads), (3, 3));
        assert_eq!((frame.batches[1].texture, frame.batches[1].quads), (4, 1));
        let depth = |q: usize| frame.vertices[q * 4].pos[2];
        assert_eq!([depth(0), depth(1), depth(2)], [900., 50., 5.]);
        // A depth range wider than the 1600 buckets is shifted down and
        // still drawn far first, with nothing dropped.
        let mut frame = Frame::default();
        let list: Vec<_> = (0..200).map(|i| particle(0, i * 100, -1)).collect();
        b.add_list(&mut frame, &list, &view(), [0; 3]);
        assert_eq!(frame.batches[0].quads, 200);
        let depths: Vec<f32> = (0..200).map(|q| frame.vertices[q * 4].pos[2]).collect();
        assert!(depths.windows(2).all(|w| w[0] >= w[1]));
        assert_eq!(frame.dropped, 0);
    }

    /// A bucket keeps 64 particles and appends the rest to an overflow list;
    /// drawing goes bucket newest-first, then its overflow list
    /// newest-first.
    #[test]
    fn full_buckets_spill_into_overflow_lists() {
        let mut frame = Frame::default();
        let mut b = Builder::default();
        // One depth bucket; x tells the particles apart.
        let list: Vec<_> = (0..100).map(|i| particle(i, 0, -1)).collect();
        b.add_list(&mut frame, &list, &view(), [0; 3]);
        assert_eq!(frame.batches[0].quads, 100);
        assert_eq!(frame.dropped, 0);
        // Corner 0 sits `size` (8) left of the particle's x.
        let drawn: Vec<i32> = (0..100)
            .map(|q| frame.vertices[q * 4].pos[0] as i32 + 8)
            .collect();
        let expected: Vec<i32> = (0..64).rev().chain((64..100).rev()).collect();
        assert_eq!(drawn, expected);
    }

    #[test]
    fn rotated_quads_use_the_rotation_about_the_view_axis() {
        let mut frame = Frame::default();
        let mut b = Builder::default();
        let mut p = particle(0, 0, -1);
        p.angle = 4096; // 90 degrees
        b.add_list(&mut frame, &[p], &view(), [0; 3]);
        // r = s * up, u = -s * right: corner(-1,-1) = -r - u = (8, -8).
        let c = frame.vertices[0].pos;
        assert!((c[0] - 8.0).abs() < 1e-3 && (c[1] + 8.0).abs() < 1e-3);
    }

    #[test]
    fn owner_segments_follow_their_entity_meshes() {
        let mut b = Builder::default();
        let mut frame = Frame::default();
        // Owner at plan position 1 of the transparent list; an empty list
        // adds no segment (no drawParticles batches).
        b.add_segment(&mut frame, 1, 1, &[particle(0, 0, -1)], &view(), [0; 3]);
        b.add_segment(&mut frame, 0, 0, &[], &view(), [0; 3]);
        assert_eq!(frame.segments.len(), 1);
        let mut segment = frame.segments[0].clone();
        assert_eq!(segment.batches, 0..1);
        // Entity 0 has two meshes (shadow + body), entity 1 one.
        segment.resolve([&[], &[2, 3, 3]]);
        assert_eq!(segment.mesh_end, 3);
        segment.resolve([&[], &[2]]);
        assert_eq!(
            segment.mesh_end,
            usize::MAX,
            "an owner outside the plan draws last"
        );
    }
}
