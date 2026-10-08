//! The billboard and particle quads' GPU state and their place in the
//! forward pass (renderer plan M9; the sets are [`crate::sprites::billboards`]'
//! and [`crate::sprites::particles`]').
//!
//! # Where they draw: the modern forward groups
//!
//! The modern client does not draw a model's billboards or an owner's
//! particles with their owner, as the classic renderer does. Its forward
//! lighting pass draws the drawable queue's group 0 (opaque, no blending),
//! then group 1, resolves depth, then group 2 (alpha blended: `SRC_ALPHA,
//! ONE_MINUS_SRC_ALPHA` for colour, `ONE, ONE_MINUS_SRC_ALPHA` for alpha, no
//! alpha-to-coverage), each group sorted by its key.
//!
//! - A particle system is one group-2 drawable of the forward pass only,
//!   keyed by its particles' mean view depth, far first; no shadow,
//!   reflection or pre-pass entry.
//! - A model's billboards are one drawable per model: group 2 when their
//!   material blends (keyed far first by the model's depth, the blended
//!   models' key format), group 0 when it is opaque or alpha tested.
//! - Both draw with the depth test on (`LESS`) and depth writes **off**, no
//!   culling of their camera-facing quads, no alpha-to-coverage, no depth
//!   texture (the sprite shaders have no soft particles).
//!
//! Here: the alpha-tested billboards draw after the opaque entities and the
//! floors (the end of group 0; unblended, their faithful alpha test); the
//! blended billboards and the particle systems join the transparent
//! entities, each placed far first by its depth among them (group 2): the
//! transparent entities keep the faithful plan order (the classic far-first
//! list) and each sprite group goes before the first transparent entity
//! nearer than it. A model's billboards keep their model order, a system's
//! particles the faithful far-first bucket order.
//!
//! # Shading
//!
//! Unlit: the vertex colour (the display-referred face or particle colour,
//! through the 2.2 decode) times the material texture (sRGB decoded), the
//! material's HDR scale `1 + 31 * aux` when it has an aux map (the
//! secondary atlas, the glow's emissive boost), then the distance fog, into
//! the HDR target before the tonemap. The alpha tests are the faithful ones
//! (`> ref` for an alpha-tested billboard or an `ALPHA_TESTED` particle
//! material, else `> 0`; the modern shaders test `> 0.5 / 255`).

use crate::frame::*;

/// One sprite corner (vertex stream 0 of the sprite pipelines).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct SpriteVertex {
    /// Camera-local position.
    pub(crate) pos: [f32; 3],
    /// R, G, B, A bytes (display-referred colour, opacity).
    pub(crate) colour: [u8; 4],
    pub(crate) uv: [f32; 2],
    /// x: the alpha reference (the fragment is kept above it), y: 1 = the
    /// material's HDR scale applies, z/w: unused.
    pub(crate) params: [f32; 4],
}

/// Which sprite pipeline a draw uses (both test depth without writing it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SpritePipeline {
    /// Alpha blended (group 2).
    Blend,
    /// Alpha tested, unblended (group 0).
    Cutout,
}

/// One sprite draw: `quads` quads from `first_quad` with one material and
/// pipeline, drawn after the first `after` model draws of the frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SpriteDraw {
    pub(crate) after: usize,
    pub(crate) first_quad: u32,
    pub(crate) quads: u32,
    pub(crate) material: i32,
    pub(crate) pipeline: SpritePipeline,
}

/// One drawable of the forward groups: a model's billboards or a particle
/// system, its depth (classic view z, larger is farther) and draws (without
/// their position yet).
pub(crate) struct Group {
    pub(crate) depth: f32,
    pub(crate) pipeline: SpritePipeline,
    pub(crate) draws: Vec<(u32, u32, i32)>,
}

/// The view depth of camera-local `p` (classic view entries, the target as
/// the origin).
pub(crate) fn view_depth(view: &[f32; 16], p: [f32; 3]) -> f32 {
    p[2] * view[10] + p[1] * view[6] + p[0] * view[2] + view[14]
}

impl ModernRenderer {
    /// This frame's billboard and particle draws (see the module docs):
    /// `transparent` holds each transparent entity draw's model-draw range
    /// and matrix (camera-local), `group0_end` the number of model draws
    /// before the transparent list.
    pub(crate) fn prepare_sprites(
        &mut self,
        prep: &PrepareFrame<'_, '_>,
        list: &DrawList<'_>,
        group0_end: usize,
        transparent: &[(std::ops::Range<usize>, [f32; 16])],
    ) {
        let PrepareFrame {
            device,
            queue,
            snapshot,
            origin,
        } = *prep;
        use crate::sprites::billboards::{of_entity, Billboards, View};
        self.frame_resources.sprites.clear();
        self.frame_resources.sprite_segments.clear();
        let view = View::new(snapshot, self.preparation.faithful_bloom);
        // The billboard set (the faithful one, sprites::billboards), the loc
        // models' cached with their mesh.
        let mut billboards = Billboards::default();
        for (index, draws) in [&list.opaque, &list.transparent].into_iter().enumerate() {
            for (i, entity) in draws.iter().enumerate() {
                if entity.model.billboards.is_none() {
                    continue;
                }
                let cached = entity
                    .key
                    .and_then(|key| self.static_model(&key))
                    .and_then(|s| s.billboards.as_ref());
                let fresh;
                let (b, matrix) = match cached {
                    Some((b, matrix)) => (b, *matrix),
                    None => {
                        fresh = of_entity(snapshot, entity);
                        match &fresh {
                            Some((b, matrix)) => (b, *matrix),
                            None => continue,
                        }
                    }
                };
                billboards.add(index as u8, i + 1, b, &local_matrix(&matrix, origin), &view);
            }
        }
        let mut groups: Vec<Group> = Vec::new();
        for segment in &billboards.segments {
            let quads = &billboards.quads[segment.quads.clone()];
            let mut group = Group {
                depth: quads.iter().map(|q| q.view_depth).sum::<f32>() / quads.len() as f32,
                pipeline: if quads[0].alpha_ref == 0 {
                    SpritePipeline::Blend
                } else {
                    SpritePipeline::Cutout
                },
                draws: Vec::new(),
            };
            for q in quads {
                self.device_resources.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    snapshot.materials,
                    q.material,
                );
                // z: a model billboard (M8: its coverage squared with the
                // post chain on, `fs_sprite`).
                // w: the blended pipeline's premultiplied output (lane
                // Q-FID's `fs_sprite`; the pre-lane shader ignores it).
                let params = [
                    f32::from(q.alpha_ref) / 255.0,
                    self.hdr_scale(q.material),
                    1.0,
                    if group.pipeline == SpritePipeline::Blend {
                        1.0
                    } else {
                        0.0
                    },
                ];
                let quad = self.frame_resources.sprites.push_quad(
                    q.corners,
                    crate::billboard::QUAD_CORNERS,
                    q.colour,
                    params,
                );
                match group.draws.last_mut() {
                    Some((first, n, material))
                        if *material == q.material && *first + *n == quad =>
                    {
                        *n += 1;
                    }
                    _ => group.draws.push((quad, 1, q.material)),
                }
            }
            self.frame_resources.sprite_segments.push(SpriteSegment {
                particles: false,
                list: segment.list,
                at: segment.at,
                quads: quads.len(),
                drawn: quads.len(),
            });
            groups.push(group);
        }
        // The particle systems (the faithful frame's owners, sprites::particles).
        let placements =
            crate::sprites::particles::placements(snapshot, list, &self.frame_resources.particles);
        let mut batches = 0;
        for (list_index, at, range) in placements {
            let mut group = Group {
                depth: 0.0,
                pipeline: SpritePipeline::Blend,
                draws: Vec::new(),
            };
            let (mut depth, mut quads, mut drawn) = (0.0, 0_usize, 0_usize);
            for batch in &self.frame_resources.particles.batches[range] {
                self.device_resources.textures.ensure(
                    device,
                    queue,
                    snapshot.pack,
                    snapshot.materials,
                    batch.texture,
                );
                // An `ALPHA_TESTED` material's threshold is the alpha test.
                let threshold = (batch.texture >= 0)
                    .then(|| snapshot.materials.and_then(|m| m.get(batch.texture as u32)))
                    .flatten()
                    .filter(|m| m.alpha == crate::texture::AlphaMode::AlphaTested)
                    .map_or(0, |m| m.alpha_threshold);
                let params = [
                    f32::from(threshold) / 255.0,
                    self.hdr_scale(batch.texture),
                    0.0,
                    1.0,
                ];
                let first = batch.first_vertex as usize;
                let Some(corners) = self
                    .frame_resources
                    .particles
                    .vertices
                    .get(first..first + batch.quads as usize * 4)
                else {
                    continue;
                };
                let mut first_quad = None;
                // The per-emitter order and size skip
                // (crate::sprites::particles::draw_order; the faithful order when off).
                let order =
                    crate::sprites::particles::draw_order(corners, |c| view_depth(&view.view, c));
                let mut n = 0_u32;
                for &k in &order {
                    let v = &corners[k * 4..k * 4 + 4];
                    let centre: [f32; 3] =
                        std::array::from_fn(|i| v.iter().map(|c| c.pos[i]).sum::<f32>() * 0.25);
                    depth += view_depth(&view.view, centre);
                    n += 1;
                    let quad = self.frame_resources.sprites.push_quad(
                        [v[0].pos, v[1].pos, v[2].pos, v[3].pos],
                        [v[0].uv, v[1].uv, v[2].uv, v[3].uv],
                        v[0].colour,
                        params,
                    );
                    first_quad.get_or_insert(quad);
                }
                quads += batch.quads as usize;
                if let Some(first_quad) = first_quad {
                    group.draws.push((first_quad, n, batch.texture));
                    drawn += n as usize;
                    batches += 1;
                }
            }
            self.frame_resources.sprite_segments.push(SpriteSegment {
                particles: true,
                list: list_index,
                at,
                quads,
                drawn,
            });
            if drawn > 0 {
                group.depth = depth / drawn as f32;
                groups.push(group);
            }
        }
        // Group 0's tail: the alpha-tested billboards after the opaque
        // entities and floors. Group 2: the blended groups far first,
        // merged into the transparent entities' order by depth.
        let mut blended: Vec<&Group> = Vec::new();
        for group in &groups {
            match group.pipeline {
                SpritePipeline::Cutout => {
                    for &(first, n, material) in &group.draws {
                        self.frame_resources.sprites.add_draw(
                            group0_end,
                            first,
                            n,
                            material,
                            SpritePipeline::Cutout,
                        );
                    }
                }
                SpritePipeline::Blend => blended.push(group),
            }
        }
        blended.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        let mut next = 0;
        let end = transparent.last().map_or(group0_end, |(r, _)| r.end);
        for (range, matrix) in transparent {
            if range.is_empty() {
                continue;
            }
            let depth = view_depth(&view.view, [matrix[12], matrix[13], matrix[14]]);
            while blended.get(next).is_some_and(|g| g.depth >= depth) {
                for &(first, n, material) in &blended[next].draws {
                    self.frame_resources.sprites.add_draw(
                        range.start,
                        first,
                        n,
                        material,
                        SpritePipeline::Blend,
                    );
                }
                next += 1;
            }
        }
        for group in &blended[next..] {
            for &(first, n, material) in &group.draws {
                self.frame_resources.sprites.add_draw(
                    end.max(group0_end),
                    first,
                    n,
                    material,
                    SpritePipeline::Blend,
                );
            }
        }
        self.stats.billboards = billboards.quads.len();
        self.stats.billboard_owners = billboards.segments.len();
        self.stats.particles = self
            .frame_resources
            .sprite_segments
            .iter()
            .filter(|s| s.particles)
            .map(|s| s.quads)
            .sum();
        self.stats.particle_batches = batches;
        self.stats.hdr_sprites = self
            .frame_resources
            .sprites
            .frame
            .chunks_exact(4)
            .filter(|q| q[0].params[1] > 0.5)
            .count();
        self.frame_resources.billboards = billboards;
        self.frame_resources.sprites.upload(device, queue);
    }

    /// 1 when `material` has an aux map (the HDR scale), else 0.
    pub(crate) fn hdr_scale(&self, material: i32) -> f32 {
        let aux = self
            .device_resources
            .textures
            .get(material)
            .is_some_and(|m| m.info.flags & crate::models::materials::FLAG_AUX != 0);
        f32::from(u8::from(aux))
    }
}

/// See the module docs (the pipelines are the forward-target set's,
/// `frame::pipelines`).
#[derive(Default)]
pub(crate) struct SpriteGpu {
    pub(crate) vertices: Option<(wgpu::Buffer, u64)>,
    pub(crate) indices: Option<(wgpu::Buffer, u32)>,
    /// The frame's corners and draws (in draw order).
    pub(crate) frame: Vec<SpriteVertex>,
    pub(crate) draws: Vec<SpriteDraw>,
}

/// The sprite vertex layout.
pub(crate) fn sprite_layout() -> wgpu::VertexBufferLayout<'static> {
    const A: [wgpu::VertexAttribute; 4] = wgpu::vertex_attr_array![
        0 => Float32x3, 3 => Unorm8x4, 2 => Float32x2, 8 => Float32x4
    ];
    wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<SpriteVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &A,
    }
}

impl SpriteGpu {
    /// A sprite pipeline over the forward pass's `layout` and shader
    /// `module` (entry points `vs_sprite`/`fs_sprite`) at `samples`:
    /// `blended`, else cutout. Group 2's blend is the forward pass's
    /// (`ALPHA_BLEND`); premultiplied, as the particle shading is: the same
    /// result for the blended billboards.
    pub(crate) fn pipeline(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
        samples: u32,
        blended: bool,
    ) -> wgpu::RenderPipeline {
        let blend = blended.then(crate::models::shading::sprite_blend);
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("modern sprites"),
            layout: Some(layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_sprite"),
                buffers: &[Some(sprite_layout())],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_sprite"),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
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
            // BuiltinStates+564: the test on (LESS), no depth writes.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
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
    }

    pub(crate) fn clear(&mut self) {
        self.frame.clear();
        self.draws.clear();
    }

    /// Append one quad's four corners (`corners` in the fan order of
    /// `uvs`); returns its quad index.
    pub(crate) fn push_quad(
        &mut self,
        corners: [[f32; 3]; 4],
        uvs: [[f32; 2]; 4],
        colour: [u8; 4],
        params: [f32; 4],
    ) -> u32 {
        let quad = (self.frame.len() / 4) as u32;
        for (pos, uv) in corners.into_iter().zip(uvs) {
            self.frame.push(SpriteVertex {
                pos,
                colour,
                uv,
                params,
            });
        }
        quad
    }

    /// Add a draw after the first `after` model draws (draws at one point
    /// keep the order they are added in).
    pub(crate) fn add_draw(
        &mut self,
        after: usize,
        first_quad: u32,
        quads: u32,
        material: i32,
        pipeline: SpritePipeline,
    ) {
        let at = self.draws.partition_point(|d| d.after <= after);
        self.draws.insert(
            at,
            SpriteDraw {
                after,
                first_quad,
                quads,
                material,
                pipeline,
            },
        );
    }

    /// Upload the frame's corners and grow the quad index buffer (two
    /// triangles per quad, `(0, 1, 2)` and `(0, 2, 3)`: the fan both
    /// faithful quad paths draw).
    pub(crate) fn upload(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        if self.frame.is_empty() {
            return;
        }
        let need = (self.frame.len() * std::mem::size_of::<SpriteVertex>()) as u64;
        if self.vertices.as_ref().is_none_or(|(_, cap)| *cap < need) {
            let cap = need.next_power_of_two();
            self.vertices = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("modern sprite vertices"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }),
                cap,
            ));
        }
        let (buffer, _) = self.vertices.as_ref().expect("sprite vertices");
        queue.write_buffer(buffer, 0, bytemuck::cast_slice(&self.frame));
        let quads = (self.frame.len() / 4) as u32;
        if self.indices.as_ref().is_none_or(|(_, cap)| *cap < quads) {
            let cap = quads.next_power_of_two().max(64);
            let indices: Vec<u32> = (0..cap)
                .flat_map(|q| {
                    let v = q * 4;
                    [v, v + 1, v + 2, v, v + 2, v + 3]
                })
                .collect();
            self.indices = Some((
                wgpu::util::DeviceExt::create_buffer_init(
                    device,
                    &wgpu::util::BufferInitDescriptor {
                        label: Some("modern sprite quads"),
                        contents: bytemuck::cast_slice(&indices),
                        usage: wgpu::BufferUsages::INDEX,
                    },
                ),
                cap,
            ));
        }
    }

    /// Draw `draw` (the forward pass's groups 0, 2 and 3 bound; group 1 is
    /// the draw's material).
    pub(crate) fn draw<'p>(
        &'p self,
        pass: &mut wgpu::RenderPass<'p>,
        pipelines: &'p [wgpu::RenderPipeline; 2],
        textures: &'p Textures,
        draw: &SpriteDraw,
    ) {
        let (Some((vertices, _)), Some((indices, _)), Some(material)) = (
            self.vertices.as_ref(),
            self.indices.as_ref(),
            textures.get(draw.material),
        ) else {
            return;
        };
        pass.set_pipeline(&pipelines[usize::from(draw.pipeline == SpritePipeline::Cutout)]);
        pass.set_bind_group(1, &material.bind_group, &[]);
        // The sprite pipelines share the forward layout, group 4 included.
        pass.set_bind_group(4, &textures.arrays.bind, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
        let first = draw.first_quad * 6;
        pass.draw_indexed(first..first + draw.quads * 6, 0, 0..1);
    }
}
