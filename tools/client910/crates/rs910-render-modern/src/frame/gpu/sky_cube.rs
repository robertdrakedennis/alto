//! The GPU half of the sky ([`crate::atmosphere::sky`]): the cube maps, their cross-fade and the
//! exposure offset.
//!
//! The 910 cache holds no cube-map sky, so each cube is baked from the environment's existing
//! sky box (module [`crate::atmosphere::sky`], "This port"): the box's dome model, or its tiled
//! material, drawn once from the origin into the six faces of an `Rgba16Float` cube in the
//! classic sky's display-referred colour (drawn at the sky layers' exposure 1), over the frame's
//! fog colour. Each box the environment names, and each box the snapshot's layers draw, is
//! baked when first seen (and again when what it is made of changes: a model that failed to
//! load and the material path that takes over). The baking submits its own small command
//! buffer after the frame's uploads; it is not a pass of the frame.
//!
//! Each frame, [`ModernRenderer::plan_sky_cubes`] feeds the environment's cube (the snapshot's
//! [`rs910_scene::sky_frame::SkyFrame::target`]) to the fade ([`SkyState`]) and plans the bakes
//! that are due; [`ModernRenderer::bind_sky_cubes`] binds the two cubes the picture mixes.

use crate::atmosphere::sky_fade::{Blend, SkyInput, SkyState};
use crate::frame::gpu::sky_layers::{SkyLayerUniforms, SkyTextureKey};
use crate::frame::*;

/// One face's size in texels.
///
/// 512 texels: the cube is a smooth, low-frequency stand-in sampled with a linear filter, and
/// the frames with the box at 256, 512 and 1024 (aerial, along the street, looking up, at
/// 1600x1000) differ by at most one 8-bit step at 512 (three at 256); 1024 was 50 MB a cube.
pub(crate) const CUBE_RES: u32 = 512;
/// The faces' format: the sky's HDR colour.
pub(crate) const CUBE_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// How many baked cubes are kept: the fading pair, the boxes the snapshot names and a spare.
/// Cubes past that which nothing uses are dropped (a cube is 12.6 MB).
const KEPT_CUBES: usize = 4;
/// The faces' near and far planes (classic units; the sky dome is some 8,000 units wide).
const FACE_NEAR: f32 = 16.0;
const FACE_FAR: f32 = 100_000.0;
const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

/// What a baked cube was made of: a dome model drawn, a material layer drawn (each only when its
/// data was there). A cube is baked again when this changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Content {
    pub(crate) model: bool,
    pub(crate) material: bool,
}

/// A baked cube.
pub(crate) struct SkyCube {
    pub(crate) content: Content,
    pub(crate) view: wgpu::TextureView,
    _texture: wgpu::Texture,
}

/// One cube waiting to be baked this frame: its layers per face (slots of `layers`) and its dome
/// model draws (their instances are in the frame's instance buffer, identity model matrix).
struct Pending {
    key: crate::skybox::SkyboxKey,
    content: Content,
    layers: [Vec<(u32, Option<SkyTextureKey>)>; 6],
    models: Vec<Draw>,
}

/// The pipeline that shades the sky into a capture face: one sample, the HDR format, over the
/// face's depth attachment.
struct FacePipeline {
    pipeline: wgpu::RenderPipeline,
    frame_layout: wgpu::BindGroupLayout,
    pass_layout: wgpu::BindGroupLayout,
    /// Stands in for the decor source (a capture draws no decor sprites).
    decor: wgpu::TextureView,
}

/// A capture face the sky is shaded into.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FaceView {
    /// The face of the cube seen from the origin (`lighting::probes::face_axes`).
    pub(crate) face: usize,
    /// Its near and far planes.
    pub(crate) limits: (f32, f32),
    /// Its size in texels.
    pub(crate) res: u32,
    /// The exposure offset added last: the ambient capture's
    /// (`lighting::ambient::CAPTURE_EXPOSURE`).
    pub(crate) exposure: f32,
}

/// A capture face's sky: what [`ModernRenderer::draw_sky_face`] draws.
pub(crate) struct SkyFace {
    frame: wgpu::BindGroup,
    pass: wgpu::BindGroup,
}

/// The resources the cubes need once there is a device.
pub(crate) struct CubeGpu {
    face: Option<FacePipeline>,
    sampler: wgpu::Sampler,
    /// Stands in for a side of the mix that has no cube.
    dummy: wgpu::TextureView,
    /// The bind group of the two cubes the frame mixes and what it was made for.
    bind: Option<(wgpu::BindGroup, [Option<crate::skybox::SkyboxKey>; 2], u64)>,
}

/// The sky's cubes and their timeline (see the module docs).
#[derive(Default)]
pub(crate) struct SkyCubes {
    cubes: HashMap<crate::skybox::SkyboxKey, SkyCube>,
    state: SkyState<crate::skybox::SkyboxKey>,
    pending: Vec<Pending>,
    /// Each face's layer uniforms of this frame's bakes.
    layers: Vec<SkyLayerUniforms>,
    /// The face windings of this frame's bakes (mirrored faces use the clockwise pipelines).
    winding: [usize; 6],
    /// How many cubes were baked so far (a bake replaces a cube's texture).
    epoch: u64,
    /// This frame's picture.
    pub(crate) blend: Option<Blend<crate::skybox::SkyboxKey>>,
    gpu: Option<CubeGpu>,
    /// The exposure offset added to the sky (0 except while the ambient capture draws).
    pub(crate) exposure_offset: f32,
    /// Tests: the environment's cube, whether it arrives through a transition and the
    /// override's fade duration, instead of the snapshot's.
    #[cfg(test)]
    pub(crate) forced: Option<(Option<crate::skybox::SkyboxKey>, bool, Option<u32>)>,
    /// Tests: the cubes baked so far.
    #[cfg(test)]
    pub(crate) bakes: usize,
    /// Tests: the bind groups made.
    #[cfg(test)]
    pub(crate) binds: usize,
}

impl SkyCubes {
    /// How many cubes were baked so far (and dropped: a cube that changes bumps it).
    pub(crate) fn baked(&self) -> u64 {
        self.epoch
    }

    /// Whether `key` is baked, or is baked this frame.
    fn ready(&self, key: crate::skybox::SkyboxKey) -> bool {
        self.cubes.contains_key(&key) || self.pending.iter().any(|p| p.key == key)
    }

    /// How much of the decor models of `key` show this frame ([`Blend::weight`]); none without a
    /// sky.
    pub(crate) fn decor_weight(&self, key: crate::skybox::SkyboxKey) -> f32 {
        self.blend.as_ref().map_or(0.0, |b| b.weight(key))
    }

    /// The sky shading's inputs for this frame (the cubes that exist, the blend, the levels).
    pub(crate) fn shading(
        &self,
        level: f32,
        decor: bool,
        flat: [f32; 3],
        exposure: f32,
    ) -> crate::atmosphere::sky::SkyShading {
        let has =
            |k: Option<crate::skybox::SkyboxKey>| k.is_some_and(|k| self.cubes.contains_key(&k));
        let blend = self.blend.unwrap_or(Blend {
            previous: None,
            current: None,
            t: 0.0,
            mixing: false,
        });
        let (previous, current) = (has(blend.previous), has(blend.current));
        let cube = if blend.mixing {
            previous || current
        } else {
            current
        };
        crate::atmosphere::sky::SkyShading {
            cube,
            decor: decor && cube,
            level,
            previous,
            current,
            mixing: blend.mixing,
            t: blend.t,
            exposure,
            flat,
        }
    }
}

impl CubeGpu {
    fn new(device: &wgpu::Device, queue: &dyn rs910_gpu_device::uploads::Uploader) -> Self {
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("modern sky cube"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let dummy = device
            .create_texture_with_data(
                queue.queue(),
                &wgpu::TextureDescriptor {
                    label: Some("modern sky cube dummy"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 6,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: CUBE_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0; 8 * 6],
            )
            .create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            });
        Self {
            face: None,
            sampler,
            dummy,
            bind: None,
        }
    }
}

impl FacePipeline {
    fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        module: &wgpu::ShaderModule,
    ) -> Self {
        let uniform = |dynamic, size: u64| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: wgpu::BufferSize::new(size),
            },
            count: None,
        };
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern sky face frame"),
            entries: &[uniform(false, std::mem::size_of::<FrameUniforms>() as u64)],
        });
        let pass_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern sky face pass"),
            entries: &[
                uniform(
                    false,
                    std::mem::size_of::<crate::atmosphere::sky::PassUniforms>() as u64,
                ),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let cubes = cube_layout(device);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern sky face"),
            bind_group_layouts: &[Some(&frame_layout), Some(&pass_layout), None, Some(&cubes)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("modern sky face"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_full"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_sky"),
                targets: &[Some(HDR_FORMAT.into())],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });
        let decor = device
            .create_texture_with_data(
                queue.queue(),
                &wgpu::TextureDescriptor {
                    label: Some("modern sky face decor"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: HDR_FORMAT,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0; 8],
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        Self {
            pipeline,
            frame_layout,
            pass_layout,
            decor,
        }
    }
}

/// The bind group layout of the sky shading's cubes (group 3: the previous and current cube and
/// their sampler).
pub(crate) fn cube_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let cube = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: wgpu::TextureViewDimension::Cube,
            multisampled: false,
        },
        count: None,
    };
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("modern sky cubes"),
        entries: &[
            cube(0),
            cube(1),
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            },
        ],
    })
}

impl ModernRenderer {
    /// Add `x` to the sky's colour last: 0 in every normal frame, the ambient capture's exposure
    /// while it draws (and 0 again after it).
    pub fn set_sky_exposure_offset(&mut self, x: f32) {
        self.sky_cubes.exposure_offset = x;
    }

    /// The sky of a capture face as this frame's sky shows it (the same cubes, blend, fog, glow
    /// and level), at the face's exposure offset. `frame` is the frame's unpacked block; the
    /// face's block takes the frame's scattering.
    pub(crate) fn prepare_sky_face(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        frame: &FrameUniforms,
        view: FaceView,
    ) -> SkyFace {
        let FaceView {
            face,
            limits,
            res,
            exposure,
        } = view;
        let module = self.shaders.get(
            device,
            crate::shaders::Module::Atmosphere {
                multisampled: false,
            },
        );
        let gpu = self
            .sky_cubes
            .gpu
            .get_or_insert_with(|| CubeGpu::new(device, queue));
        let pipeline = gpu
            .face
            .get_or_insert_with(|| FacePipeline::new(device, queue, &module));
        let (frame_layout, pass_layout) =
            (pipeline.frame_layout.clone(), pipeline.pass_layout.clone());
        let block = crate::frame::gpu::probes::capture_frame(
            &self.scattering_packed(frame),
            [0.0; 3],
            face,
            limits,
            true,
        );
        let inv = glam::Mat4::from_cols_array_2d(&block.view_proj)
            .inverse()
            .to_cols_array_2d();
        let (slot, _) = self.sky_slot(
            &block,
            inv,
            [0.0, 0.0, res as f32, res as f32],
            exposure,
            false,
        );
        let buffer = |label, contents: &[u8]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents,
                usage: wgpu::BufferUsages::UNIFORM,
            })
        };
        let frame_buffer = buffer("modern sky face frame", bytemuck::bytes_of(&block));
        let pass_buffer = buffer("modern sky face pass", bytemuck::bytes_of(&slot));
        let decor = &self
            .sky_cubes
            .gpu
            .as_ref()
            .and_then(|g| g.face.as_ref())
            .expect("the face pipeline")
            .decor;
        SkyFace {
            frame: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern sky face frame"),
                layout: &frame_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame_buffer.as_entire_binding(),
                }],
            }),
            pass: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern sky face pass"),
                layout: &pass_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: pass_buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(decor),
                    },
                ],
            }),
        }
    }

    /// Draw a capture face's sky ([`Self::prepare_sky_face`]) into `pass`, which covers the
    /// whole face (HDR colour, one sample, a depth attachment it leaves alone).
    pub(crate) fn draw_sky_face<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>, sky: &'p SkyFace) {
        let (Some(face), Some(cubes)) = (
            self.sky_cubes.gpu.as_ref().and_then(|g| g.face.as_ref()),
            self.sky_cube_bind(),
        ) else {
            return;
        };
        pass.set_pipeline(&face.pipeline);
        pass.set_bind_group(0, &sky.frame, &[]);
        pass.set_bind_group(1, &sky.pass, &[]);
        pass.set_bind_group(3, cubes, &[]);
        pass.draw(0..3, 0..1);
    }

    /// This frame's sky cubes: the environment's cube follows its fade, and the boxes due for a
    /// bake are planned (their geometry and layers recorded, drawn by [`Self::bake_sky_cubes`]
    /// once the frame's buffers are uploaded). `main_view_proj` is the frame's camera clip
    /// matrix (its handedness decides which faces draw clockwise) and `now_ms` the frame's
    /// clock.
    pub(crate) fn plan_sky_cubes(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        main_view_proj: &[[f32; 4]; 4],
        now_ms: i64,
    ) {
        use crate::skybox::SkyLayer;
        let cubes = &mut self.sky_cubes;
        cubes.pending.clear();
        cubes.layers.clear();
        if cubes.gpu.is_none() {
            cubes.gpu = Some(CubeGpu::new(device, queue));
        }
        let sky = snapshot.sky;
        let target = sky.and_then(|s| s.target());
        // The boxes to bake: the environment's and every box the layers draw.
        let mut keys: Vec<crate::skybox::SkyboxKey> = target.into_iter().collect();
        if let Some(sky) = sky {
            for layer in sky.layers {
                if let SkyLayer::Model { key, .. } | SkyLayer::Material { key, .. } = layer {
                    if !keys.contains(key) {
                        keys.push(*key);
                    }
                }
            }
        }
        let main = glam::Mat4::from_cols_array_2d(main_view_proj).determinant();
        self.sky_cubes.winding = std::array::from_fn(|face| {
            let d = crate::lighting::probes::face_view_proj(face, [0.0; 3], FACE_NEAR, 1000.0)
                .determinant();
            usize::from((d > 0.0) != (main > 0.0))
        });
        if let Some(sky) = sky {
            for &key in &keys {
                self.plan_bake(device, queue, snapshot, &sky, key);
            }
        }
        let cubes = &mut self.sky_cubes;
        let observed = (
            target,
            sky.is_some_and(|s| s.fading()),
            sky.and_then(|s| s.fade_override_ms()),
        );
        #[cfg(test)]
        let observed = cubes.forced.unwrap_or(observed);
        let (target, transition, duration_override_ms) = observed;
        let input = SkyInput {
            target,
            ready: target.is_none_or(|k| cubes.ready(k)),
            transition,
            duration_override_ms,
        };
        let blend = cubes.state.update(input, now_ms);
        cubes.blend = Some(blend);
        if cubes.cubes.len() > KEPT_CUBES {
            let mut in_use = keys;
            in_use.extend(
                [blend.previous, blend.current, target]
                    .into_iter()
                    .flatten(),
            );
            let unused: Vec<_> = cubes
                .cubes
                .keys()
                .copied()
                .filter(|k| !in_use.contains(k))
                .collect();
            for key in unused.into_iter().take(cubes.cubes.len() - KEPT_CUBES) {
                cubes.cubes.remove(&key);
                cubes.epoch += 1;
            }
        }
    }

    /// Plan the bake of the cube of box `key` when it is missing or made of something else now.
    fn plan_bake(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        sky: &crate::sky_frame::SkyFrame<'_>,
        key: crate::skybox::SkyboxKey,
    ) {
        use crate::skybox::SkyLayer;
        let mut content = Content {
            model: false,
            material: false,
        };
        let mut layers: [Vec<(u32, Option<SkyTextureKey>)>; 6] = Default::default();
        let mut models = Vec::new();
        let materials = snapshot.materials;
        let cam_yaw = snapshot.camera.yaw_int();
        let res = CUBE_RES as i32;
        let argb = |c: u32| {
            [
                ((c >> 16) & 0xff) as f32 / 255.0,
                ((c >> 8) & 0xff) as f32 / 255.0,
                (c & 0xff) as f32 / 255.0,
                ((c >> 24) & 0xff) as f32 / 255.0,
            ]
        };
        for layer in sky.layers {
            match layer {
                SkyLayer::Model { key: k, .. } if *k == key => {
                    let (Some(model), Some(materials)) = (sky.unfaded_model(key), materials) else {
                        continue;
                    };
                    let Some(streams) =
                        crate::models::mesh::model_streams(&model, materials, Colour::Classic)
                    else {
                        continue;
                    };
                    content.model = true;
                    let (base_vertex, first) = self.arena.push(&streams);
                    for &(material, start, count) in &streams.batches {
                        self.textures.ensure(
                            device,
                            queue,
                            snapshot.pack,
                            Some(materials),
                            material,
                        );
                        let instance = self.instances.len() as u32;
                        self.instances.push(self.instance(
                            IDENTITY,
                            material,
                            1.0,
                            FLAG_SKY | FLAG_UNLIT,
                        ));
                        models.push(Draw {
                            geometry: Geometry::Arena { base_vertex },
                            material,
                            first_index: start + first,
                            count,
                            instance,
                            pass: Pass::NoDepthWrite,
                            casts: false,
                            indirect: None,
                        });
                    }
                }
                SkyLayer::Material {
                    key: k,
                    material,
                    yaw,
                    fill,
                    fog,
                    ..
                } if *k == key => {
                    let Some(texture) = sky.sprite(key) else {
                        continue;
                    };
                    self.ensure_sky_texture(device, queue, SkyTextureKey::Material(key), texture);
                    content.material = true;
                    let multiply = materials
                        .and_then(|m| m.get(*material as u32))
                        .is_some_and(|m| m.alpha == crate::texture::AlphaMode::Multiply);
                    let horizon = *fill == Some(crate::skybox::SkyBoxFillMode::Horizon);
                    for (face, face_layers) in layers.iter_mut().enumerate() {
                        let (face_pitch, face_yaw) = crate::lighting::probes::face_angles(face);
                        let base = SkyLayerUniforms {
                            rect: [0.0, 0.0, res as f32, res as f32],
                            params: [crate::post::tonemap::EXPOSURE, 0.0, 0.0, 0.0],
                            ..SkyLayerUniforms::default()
                        };
                        let mut push = |u: SkyLayerUniforms, t| {
                            face_layers.push((self.sky_cubes.layers.len() as u32, t));
                            self.sky_cubes.layers.push(u);
                        };
                        // A multiply material over the fog colour.
                        if multiply {
                            let mut colour = argb(*fog as u32);
                            colour[3] = 1.0;
                            push(SkyLayerUniforms { colour, ..base }, None);
                        }
                        // The box's own yaw is the layer's less the camera's; the face looks
                        // along its axis.
                        let yaw = (yaw - cam_yaw + face_yaw) & 0x3FFF;
                        let mut tile_y = res.wrapping_mul(face_pitch) / -4096;
                        let mut tile_x = res.wrapping_mul(yaw) / 4096;
                        tile_x = tile_x.rem_euclid(res);
                        if !horizon {
                            tile_y = tile_y.rem_euclid(res);
                        }
                        push(
                            SkyLayerUniforms {
                                colour: [1.0, 1.0, 1.0, 1.0],
                                tile: [
                                    tile_x as f32,
                                    tile_y as f32,
                                    res as f32,
                                    if horizon { 2.0 } else { 1.0 },
                                ],
                                top: argb(texture.first as u32),
                                bottom: argb(texture.last as u32),
                                ..base
                            },
                            Some(SkyTextureKey::Material(key)),
                        );
                    }
                }
                _ => {}
            }
        }
        if self
            .sky_cubes
            .cubes
            .get(&key)
            .is_some_and(|c| c.content == content)
        {
            // Nothing new: the records made for it are not drawn.
            return;
        }
        self.sky_cubes.pending.push(Pending {
            key,
            content,
            layers,
            models,
        });
    }

    /// Draw the cubes planned this frame (after the frame's uploads): each face of the cube
    /// cleared to the fog colour, then the box's material layers and dome model over it.
    pub(crate) fn bake_sky_cubes(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        if self.sky_cubes.pending.is_empty() {
            return;
        }
        let Some(pipes) = self.probes.pipes.as_ref() else {
            // The capture pipelines are built with the renderer; none: bake next frame.
            self.sky_cubes.pending.clear();
            return;
        };
        let pending = std::mem::take(&mut self.sky_cubes.pending);
        let layer_uniforms = std::mem::take(&mut self.sky_cubes.layers);
        let winding = self.sky_cubes.winding;
        let clear = wgpu::Color {
            r: f64::from(self.clear[0]),
            g: f64::from(self.clear[1]),
            b: f64::from(self.clear[2]),
            a: 1.0,
        };
        // The faces' frames: each looks along its axis from the origin, the sky models unlit.
        let frames: Vec<FrameUniforms> = (0..6)
            .map(|face| FrameUniforms {
                view_proj: crate::lighting::probes::face_view_proj(
                    face, [0.0; 3], FACE_NEAR, FACE_FAR,
                )
                .to_cols_array_2d(),
                view: glam::Mat4::IDENTITY.to_cols_array_2d(),
                params: [crate::post::tonemap::EXPOSURE, 0.0, 0.0, 0.0],
                ..FrameUniforms::default()
            })
            .collect();
        let slot = std::mem::size_of::<FrameUniforms>() as u64;
        let frame_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern sky cube frames"),
            contents: bytemuck::cast_slice(&frames),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let frame_layout = self.pipes().forward.get_bind_group_layout(0);
        let frame_binds: Vec<wgpu::BindGroup> = (0..6)
            .map(|face| {
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("modern sky cube frame"),
                    layout: &frame_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                                buffer: &frame_buffer,
                                offset: face * slot,
                                size: wgpu::BufferSize::new(slot),
                            }),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(&pipes.ao_white),
                        },
                    ],
                })
            })
            .collect();
        let layer_bind = (!layer_uniforms.is_empty()).then(|| {
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("modern sky cube layers"),
                contents: bytemuck::cast_slice(&layer_uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern sky cube layers"),
                layout: &self.sky_layer_layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<SkyLayerUniforms>() as u64),
                    }),
                }],
            });
            (buffer, bind)
        });
        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("modern sky cube depth"),
                size: wgpu::Extent3d {
                    width: CUBE_RES,
                    height: CUBE_RES,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("modern sky cubes"),
        });
        let mut baked = Vec::new();
        for bake in &pending {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("modern sky cube"),
                size: wgpu::Extent3d {
                    width: CUBE_RES,
                    height: CUBE_RES,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CUBE_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            for face in 0..6_usize {
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D2),
                    base_array_layer: face as u32,
                    array_layer_count: Some(1),
                    ..Default::default()
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("modern sky cube face"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        resolve_target: None,
                        depth_slice: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(clear),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &depth,
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
                if let Some((_, layers)) = layer_bind.as_ref() {
                    pass.set_pipeline(&pipes.sky_layer);
                    for &(index, texture) in &bake.layers[face] {
                        pass.set_bind_group(
                            0,
                            layers,
                            &[index * std::mem::size_of::<SkyLayerUniforms>() as u32],
                        );
                        let texture = texture
                            .and_then(|k| self.sky_textures.get(&k))
                            .map_or(&self.sky_white, |t| &t.bind_group);
                        pass.set_bind_group(1, texture, &[]);
                        pass.draw(0..3, 0..1);
                    }
                }
                if !bake.models.is_empty() {
                    pass.set_pipeline(&pipes.sky_model[winding[face]]);
                    pass.set_bind_group(0, &frame_binds[face], &[]);
                    pass.set_bind_group(2, &pipes.no_shadow, &[]);
                    pass.set_bind_group(3, &self.lights.bind, &[]);
                    self.submit_all(&mut pass, &bake.models);
                }
            }
            let cube_view = texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                ..Default::default()
            });
            baked.push((
                bake.key,
                SkyCube {
                    content: bake.content,
                    view: cube_view,
                    _texture: texture,
                },
            ));
        }
        queue.submit_uploads(vec![encoder.finish()]);
        for (key, cube) in baked {
            log::info!(
                "[modern] sky cube {key:?}: baked ({}{})",
                if cube.content.model {
                    "dome model "
                } else {
                    ""
                },
                if cube.content.material {
                    "material layer"
                } else {
                    ""
                }
            );
            self.sky_cubes.cubes.insert(key, cube);
            self.sky_cubes.epoch += 1;
            #[cfg(test)]
            {
                self.sky_cubes.bakes += 1;
            }
        }
    }

    /// The bind group of the two cubes this frame's picture mixes (group 3 of the sky shading).
    pub(crate) fn bind_sky_cubes(&mut self, device: &wgpu::Device) {
        let Some(layout) = self.atmos.pipes.current().map(|p| p.sky_cubes.clone()) else {
            return;
        };
        let cubes = &mut self.sky_cubes;
        let Some(blend) = cubes.blend else {
            return;
        };
        let pick = |k: Option<crate::skybox::SkyboxKey>| k.filter(|k| cubes.cubes.contains_key(k));
        let wanted = [pick(blend.previous), pick(blend.current)];
        let epoch = cubes.epoch;
        let Some(mut gpu) = cubes.gpu.take() else {
            return;
        };
        // The made group stays while the same cubes are wanted and nothing was baked again.
        let current = gpu
            .bind
            .as_ref()
            .is_some_and(|(_, made, at)| *made == wanted && *at == epoch);
        if !current {
            let bind = {
                let view = |k: Option<crate::skybox::SkyboxKey>| {
                    k.and_then(|k| cubes.cubes.get(&k))
                        .map_or(&gpu.dummy, |c| &c.view)
                };
                device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("modern sky cubes"),
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: wgpu::BindingResource::TextureView(view(wanted[0])),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view(wanted[1])),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(&gpu.sampler),
                        },
                    ],
                })
            };
            gpu.bind = Some((bind, wanted, epoch));
            #[cfg(test)]
            {
                cubes.binds += 1;
            }
        }
        cubes.gpu = Some(gpu);
    }

    /// Whether a decor sprite was drawn for this frame's sky (into the sky's source).
    pub(crate) fn sky_decor_drawn(&self) -> bool {
        self.sky.iter().any(|d| {
            matches!(
                d,
                crate::frame::gpu::sky_layers::SkyDraw::Layer {
                    texture: Some(SkyTextureKey::Decor(..)),
                    ..
                }
            )
        })
    }

    /// The sky shading's cube bind group (`None` before the first frame).
    pub(crate) fn sky_cube_bind(&self) -> Option<&wgpu::BindGroup> {
        self.sky_cubes
            .gpu
            .as_ref()
            .and_then(|g| g.bind.as_ref())
            .map(|(bind, _, _)| bind)
    }
}

#[cfg(test)]
impl ModernRenderer {
    /// Tests: a cube of one solid colour per face (HDR, as the bake leaves it: the shading
    /// divides by the sky level), in the face order `+x -x +y -y +z -z` of the cube space.
    pub(crate) fn insert_test_cube(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        key: crate::skybox::SkyboxKey,
        faces: [[f32; 3]; 6],
    ) {
        // An f16 of a normal value (the test colours are simple fractions).
        fn f16(v: f32) -> u16 {
            let b = v.to_bits();
            let sign = ((b >> 16) & 0x8000) as u16;
            let e = ((b >> 23) & 0xff) as i32 - 127 + 15;
            if v == 0.0 || e <= 0 {
                return sign;
            }
            sign | ((e.min(30) as u16) << 10) | ((b >> 13) & 0x3ff) as u16
        }
        const SIZE: u32 = 4;
        let mut bytes = Vec::new();
        for face in faces {
            for _ in 0..SIZE * SIZE {
                for v in [face[0], face[1], face[2], 1.0] {
                    bytes.extend_from_slice(&f16(v).to_le_bytes());
                }
            }
        }
        let texture = device.create_texture_with_data(
            queue.queue(),
            &wgpu::TextureDescriptor {
                label: Some("modern test sky cube"),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 6,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: CUBE_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &bytes,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::Cube),
            ..Default::default()
        });
        self.sky_cubes.cubes.insert(
            key,
            SkyCube {
                content: Content {
                    model: false,
                    material: false,
                },
                view,
                _texture: texture,
            },
        );
        self.sky_cubes.epoch += 1;
    }
}
