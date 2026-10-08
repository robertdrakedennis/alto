//! On the GPU (renderer plan §4(s)): the scattering's
//! frame values ([`crate::atmosphere`]), the modern sky pass over the
//! classic sky's own target ([`crate::atmosphere::sky`]), and the volumetric
//! scattering ([`crate::atmosphere::volumetrics`]) and depth of field
//! ([`crate::post::dof`]) over the lit frame before the tonemap. Each is
//! skipped when its switch is off (then nothing here runs and the frame is
//! the one before the lane).
use crate::atmosphere::sky::PassUniforms;
use crate::frame::encoding::EncodeInputs;
use crate::frame::*;

/// A pass slot's size (dynamic offsets are 256-byte aligned).
pub(crate) const SLOT: u64 = 256;
pub(crate) const SLOT_SKY: u32 = 0;
pub(crate) const SLOT_VOL: u32 = 1;
pub(crate) const SLOT_DOF: u32 = 2;
pub(crate) const SLOTS: u32 = 32;
/// The depth of field's intermediate format.
pub(crate) const DOF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The volumetrics' half-size depth (the farthest of each 2x2) and their
/// half-size scattering (in-scattered light, extinction).
pub(crate) const VOL_DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
pub(crate) const VOL_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;

/// One fullscreen atmosphere pass: what it draws into and with.
#[derive(Clone, Copy)]
pub(crate) struct AtmosPass<'a> {
    which: crate::frame::passes::Pass,
    view: &'a wgpu::TextureView,
    pipeline: &'a wgpu::RenderPipeline,
    bind: &'a wgpu::BindGroup,
    slot: u32,
    shadowed: bool,
    clear: Option<[f32; 3]>,
    half: Option<[u32; 2]>,
    /// The bind group of group 3 (the sky's cubes).
    cubes: Option<&'a wgpu::BindGroup>,
}

impl<'a> AtmosPass<'a> {
    /// A pass into `view` over the scene viewport, reading `slot` of the
    /// pass block through `bind`, not shadowed and not cleared.
    pub(crate) fn new(
        which: crate::frame::passes::Pass,
        view: &'a wgpu::TextureView,
        pipeline: &'a wgpu::RenderPipeline,
        bind: &'a wgpu::BindGroup,
        slot: u32,
    ) -> Self {
        Self {
            which,
            view,
            pipeline,
            bind,
            slot,
            shadowed: false,
            clear: None,
            half: None,
            cubes: None,
        }
    }

    /// Bind the sky's cubes as group 3.
    pub(crate) fn with_cubes(mut self, cubes: &'a wgpu::BindGroup) -> Self {
        self.cubes = Some(cubes);
        self
    }

    /// Bind the shadow atlas as group 2.
    pub(crate) fn shadowed(mut self) -> Self {
        self.shadowed = true;
        self
    }

    /// Clear the target to `colour` first.
    pub(crate) fn cleared(mut self, colour: [f32; 3]) -> Self {
        self.clear = Some(colour);
        self
    }

    /// Cover `size` texels from the origin instead of the scene viewport.
    pub(crate) fn over(mut self, size: [u32; 2]) -> Self {
        self.half = Some(size);
        self
    }
}

/// The pipelines at one sample count (`frame::pipelines`).
pub(crate) struct Pipes {
    pub(crate) layout1: wgpu::BindGroupLayout,
    /// The sky shading's cubes (group 3).
    pub(crate) sky_cubes: wgpu::BindGroupLayout,
    pub(crate) sky: wgpu::RenderPipeline,
    /// The volumetrics (the half-size path): the half-size depth, the
    /// march, the bilateral apply.
    pub(crate) vol_depth: wgpu::RenderPipeline,
    pub(crate) vol_march: wgpu::RenderPipeline,
    pub(crate) vol_apply: wgpu::RenderPipeline,
    pub(crate) dof_focus: wgpu::RenderPipeline,
    pub(crate) dof_blur: wgpu::RenderPipeline,
    pub(crate) dof_spread: wgpu::RenderPipeline,
    pub(crate) dof_composite: wgpu::RenderPipeline,
    pub(crate) frame_bind: wgpu::BindGroup,
    pub(crate) slots: wgpu::Buffer,
    pub(crate) dummy: wgpu::TextureView,
}

/// The size-dependent targets.
pub(crate) struct AtmosTargets {
    pub(crate) key: ([u32; 2], u32),
    /// The depth of field's chains, made the first frame it is on (lane
    /// P4-GPU: never while it is off, the default).
    pub(crate) dof: Option<DofTargets>,
    /// The volumetrics' half-size targets (while they are on).
    pub(crate) vol: Option<VolTargets>,
}

/// The volumetrics' half-size targets (the viewport's size halved: the half-size depth and the
/// half-size scattering).
pub(crate) struct VolTargets {
    pub(crate) size: [u32; 2],
    pub(crate) depth: wgpu::TextureView,
    pub(crate) scatter: wgpu::TextureView,
}

/// The depth of field's targets: the focus and its spread, the far and near
/// blurs' ping-pong pairs.
pub(crate) struct DofTargets {
    pub(crate) focus: [wgpu::TextureView; 2],
    pub(crate) far: [wgpu::TextureView; 2],
    pub(crate) near: [wgpu::TextureView; 2],
}

/// This frame's layers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AtmosFrame {
    pub scattering: Option<crate::atmosphere::Scattering>,
    pub sky_shading: bool,
    /// Decor sprites are drawn into the sky's source and shaded over the cube.
    pub sky_decor: bool,
    pub volumetrics: bool,
    pub dof: bool,
    /// The depth of field's focal distance (view depth).
    pub focal: f32,
}

/// See the module docs.
#[derive(Default)]
pub(crate) struct AtmosGpu {
    pub(crate) state: crate::atmosphere::AtmosphereState,
    /// The pipelines per sample count (`frame::pipelines`).
    pub(crate) pipes: crate::frame::pipelines::Variants<u32, Pipes>,
    pub(crate) targets: Option<AtmosTargets>,
    pub(crate) frame: AtmosFrame,
    /// This frame's scene viewport and scissor.
    pub(crate) rect: [i32; 4],
    pub(crate) clip: [i32; 4],
    /// This frame's pass bind groups: the sky, the volumetrics, the depth
    /// of field's passes in order (slot, bind group).
    pub(crate) sky_bind: Option<wgpu::BindGroup>,
    /// The volumetrics' passes' bind groups: depth, march, apply, and the
    /// targets they were made for (frame size, samples, half size): kept
    /// while those stay.
    pub(crate) vol_bind: Option<[wgpu::BindGroup; 3]>,
    pub(crate) vol_bind_key: Option<([u32; 2], u32, [u32; 2])>,
    pub(crate) dof_binds: Vec<(u32, wgpu::BindGroup)>,
    /// How many full-frame passes over the resolved frame this frame
    /// encodes (volumetrics, the depth of field's composite): each writes
    /// the other of the frame's two HDR resolves (`Targets::hdr_views`).
    pub(crate) hdr_writes: usize,
    /// The volumetrics' result is copied back into the frame's resolve
    /// (with the depth of field, whose taps clamp to the target, not the
    /// scissor, so they read the resolve outside it).
    pub(crate) vol_copy_back: bool,
    /// Per depth-of-field pass: (slot, iteration).
    pub(crate) dof_slots: Vec<(u32, f32)>,
    /// Tests: draw the geometry through clear air.
    #[cfg(test)]
    pub(crate) test_clear_air: bool,
}

pub(crate) fn view_of(
    device: &wgpu::Device,
    label: &str,
    size: [u32; 2],
    format: wgpu::TextureFormat,
    samples: u32,
    usage: wgpu::TextureUsages,
) -> (wgpu::Texture, wgpu::TextureView) {
    let t = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size[0].max(1),
            height: size[1].max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: samples,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    });
    let v = t.create_view(&wgpu::TextureViewDescriptor::default());
    (t, v)
}

impl Pipes {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        samples: u32,
        frame_buffer: &wgpu::Buffer,
        receive_layout: &wgpu::BindGroupLayout,
        module: std::sync::Arc<wgpu::ShaderModule>,
    ) -> Self {
        let layout0 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern atmosphere frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout1 = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern atmosphere pass"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<PassUniforms>() as u64,
                        ),
                    },
                    count: None,
                },
                texture(1),
                texture(2),
                texture(3),
                texture(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: samples > 1,
                    },
                    count: None,
                },
            ],
        });
        let plain = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern atmosphere"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1)],
            immediate_size: 0,
        });
        let shadowed = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern volumetrics"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1), Some(receive_layout)],
            immediate_size: 0,
        });
        let sky_cubes = crate::frame::gpu::sky_cube::cube_layout(device);
        let sky = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("modern sky shading"),
            bind_group_layouts: &[Some(&layout0), Some(&layout1), None, Some(&sky_cubes)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str,
                        layout: &wgpu::PipelineLayout,
                        format: wgpu::TextureFormat,
                        count: u32| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_full"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(entry),
                    targets: &[Some(format.into())],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState {
                    count,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let frame_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern atmosphere frame"),
            layout: &layout0,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let slots = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern atmosphere slots"),
            size: SLOT * u64::from(SLOTS),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dummy = device
            .create_texture_with_data(
                queue.queue(),
                &wgpu::TextureDescriptor {
                    label: Some("modern atmosphere dummy"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                },
                wgpu::util::TextureDataOrder::LayerMajor,
                &[0; 8],
            )
            .create_view(&wgpu::TextureViewDescriptor::default());
        // The eight pipelines are created at once (`frame::compile`).
        let pipeline = &pipeline;
        let (plain, shadowed, sky_layout) = (&plain, &shadowed, &sky);
        // Which layout each pipeline uses: 0 plain, 1 with the shadow receive, 2 the sky's.
        let specs: [(&str, u8, wgpu::TextureFormat, u32); 8] = [
            ("fs_sky", 2, HDR_FORMAT, samples),
            ("fs_vol_depth", 0, VOL_DEPTH_FORMAT, 1),
            ("fs_vol_march", 1, VOL_FORMAT, 1),
            ("fs_vol_apply", 0, HDR_FORMAT, 1),
            ("fs_dof_focus", 0, DOF_FORMAT, 1),
            ("fs_dof_blur", 0, DOF_FORMAT, 1),
            ("fs_dof_spread", 0, DOF_FORMAT, 1),
            ("fs_dof_composite", 0, HDR_FORMAT, 1),
        ];
        let jobs: Vec<crate::frame::compile::Job<'_, wgpu::RenderPipeline>> = specs
            .map(
                |(entry, layout, format, count)| -> crate::frame::compile::Job<'_, _> {
                    Box::new(move || {
                        let layout = match layout {
                            0 => plain,
                            1 => shadowed,
                            _ => sky_layout,
                        };
                        pipeline(entry, layout, format, count)
                    })
                },
            )
            .into();
        let mut built = crate::frame::compile::all(jobs).into_iter();
        let mut next = || built.next().expect("every pipeline is built");
        Self {
            sky: next(),
            vol_depth: next(),
            vol_march: next(),
            vol_apply: next(),
            dof_focus: next(),
            dof_blur: next(),
            dof_spread: next(),
            dof_composite: next(),
            layout1,
            sky_cubes,
            frame_bind,
            slots,
            dummy,
        }
    }
}

impl ModernRenderer {
    /// The atmosphere's pipelines at the current sample count (built the
    /// first time: at renderer creation, and on an anti-aliasing change).
    pub(crate) fn select_atmos_pipes(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        let samples = self.device_resources.samples;
        if !self.frame_resources.atmos.pipes.contains(&samples) {
            let pipes = self.atmos_pipes(device, queue);
            self.frame_resources
                .atmos
                .pipes
                .insert_selected(samples, pipes);
        }
        self.frame_resources.atmos.pipes.use_key(samples);
    }

    /// The atmosphere's pipelines at the current sample count (a new set).
    pub(crate) fn atmos_pipes(
        &self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) -> Pipes {
        self.encoding_inputs().atmos_pipes(device, queue)
    }

    /// The frame uniforms the geometry passes read: `uniforms` with this
    /// frame's scattering packed in (`crate::atmosphere::pack`) when the
    /// scattering is on; `uniforms` unchanged otherwise (the probe capture
    /// keeps the unpacked block).
    pub(crate) fn atmosphere_uniforms(
        &mut self,
        snapshot: &SceneSnapshot<'_>,
        uniforms: &FrameUniforms,
    ) -> FrameUniforms {
        let target = self
            .environment
            .scattering(snapshot)
            .map_or_else(crate::atmosphere::Scattering::default, |(p, c)| {
                crate::atmosphere::Scattering::from_record(p, c)
            })
            .with_inscatter_scale(self.look.scatter_inscatter);
        let now = self.frame_millis();
        let s = self.frame_resources.atmos.state.update(target, now);
        self.frame_resources.atmos.frame.scattering = Some(s);
        // Tests: the geometry drawn through clear air (the sky and the
        // probes keep the scattering).
        #[cfg(test)]
        if self.frame_resources.atmos.test_clear_air {
            return *uniforms;
        }
        self.scattering_packed(uniforms)
    }

    /// `uniforms` with this frame's scattering packed in (the frame block the geometry
    /// passes and the sky read); unchanged before the frame's scattering is known.
    pub(crate) fn scattering_packed(&self, uniforms: &FrameUniforms) -> FrameUniforms {
        let Some(s) = self.frame_resources.atmos.frame.scattering else {
            return *uniforms;
        };
        let p = crate::atmosphere::pack(&s);
        let mut u = *uniforms;
        u.sun_dir[3] = p.sun_dir_w;
        u.sun_colour[3] = p.sun_colour_w;
        u.sky_ambient[3] = p.sky_ambient_w;
        u.ground_ambient[3] = p.ground_ambient_w;
        u.fog_range[2] = p.fog_range_zw[0];
        u.fog_range[3] = p.fog_range_zw[1];
        u
    }

    /// The sky shading's slot for a view (`inv`: its clip to camera-local matrix, `rect` its
    /// viewport) of the frame block `uniforms`: this frame's cubes and blend at `exposure`
    /// (the offset added last), the decor sprites drawn over them when `decor`, and the
    /// angle fog from the camera square's record (`lighting::environment_record`) and the
    /// fog's range. Also whether the decor sprites are shaded over the cube.
    pub(crate) fn sky_slot(
        &self,
        uniforms: &FrameUniforms,
        inv: [[f32; 4]; 4],
        rect: [f32; 4],
        exposure: f32,
        decor: bool,
    ) -> (PassUniforms, bool) {
        // The fog's end is the horizon distance.
        let fogged = uniforms.fog_range[1] > 0.0;
        // For a frame without a cube: the fog colour (the clear colour when the fog is off).
        let flat = if uniforms.fog_colour[3] > 0.0 {
            [
                uniforms.fog_colour[0],
                uniforms.fog_colour[1],
                uniforms.fog_colour[2],
            ]
        } else {
            self.frame_resources.clear
        };
        let shading =
            self.scene_resources
                .sky_cubes
                .shading(self.look.sky_exposure(), decor, flat, exposure);
        let mut slot = crate::atmosphere::sky::uniforms(inv, rect, &shading);
        if fogged {
            let start = uniforms.fog_range[0];
            let [power, offset] = crate::lighting::environment_record::angle_fog_params(
                start,
                start + 1.0 / uniforms.fog_range[1],
                self.environment_record.angle_fog,
            );
            slot.p0[0] = power;
            slot.p0[1] = offset;
        }
        (slot, shading.decor)
    }

    /// This frame's sky, volumetrics and depth of field: switches, targets,
    /// pipelines and slots.
    pub(crate) fn prepare_atmos(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        uniforms: &FrameUniforms,
        size: [u32; 2],
        rect: [i32; 4],
        clip: [i32; 4],
    ) {
        self.frame_resources.atmos.rect = rect;
        self.frame_resources.atmos.clip = clip;
        let f = &mut self.frame_resources.atmos.frame;
        f.sky_shading = true;
        f.volumetrics = self.preparation.settings.volumetrics;
        f.dof = self.preparation.settings.dof;
        self.select_atmos_pipes(device, queue);
        let key = (size, self.device_resources.samples);
        if self
            .frame_resources
            .atmos
            .targets
            .as_ref()
            .is_none_or(|t| t.key != key)
        {
            self.frame_resources.atmos.targets = Some(AtmosTargets {
                key,
                dof: None,
                vol: None,
            });
        }
        let half = crate::atmosphere::volumetrics::half_size(rect);
        if let Some(t) = self.frame_resources.atmos.targets.as_mut().filter(|t| {
            self.frame_resources.atmos.frame.volumetrics
                && t.vol.as_ref().is_none_or(|v| v.size != half)
        }) {
            let rt = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
            t.vol = Some(VolTargets {
                size: half,
                depth: view_of(
                    device,
                    "modern volumetrics depth",
                    half,
                    VOL_DEPTH_FORMAT,
                    1,
                    rt,
                )
                .1,
                scatter: view_of(device, "modern volumetrics", half, VOL_FORMAT, 1, rt).1,
            });
        }
        if let Some(t) = self
            .frame_resources
            .atmos
            .targets
            .as_mut()
            .filter(|t| t.dof.is_none())
        {
            if self.frame_resources.atmos.frame.dof {
                let rt =
                    wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
                let pair =
                    |label: &str| [0, 1].map(|_| view_of(device, label, size, DOF_FORMAT, 1, rt).1);
                t.dof = Some(DofTargets {
                    focus: pair("modern dof focus"),
                    far: pair("modern dof far"),
                    near: pair("modern dof near"),
                });
            }
        }
        let vp = glam::Mat4::from_cols_array_2d(&uniforms.view_proj);
        let inv = vp.inverse().to_cols_array_2d();
        let r = rect.map(|v| v as f32);
        let mut slots = vec![PassUniforms::default(); SLOTS as usize];
        // The sky shading over the scene viewport.
        let (sky_slot, decor) = self.sky_slot(
            uniforms,
            inv,
            r,
            self.scene_resources.sky_cubes.exposure_offset,
            self.sky_decor_drawn(),
        );
        slots[SLOT_SKY as usize] = sky_slot;
        self.frame_resources.atmos.frame.sky_decor = decor;
        let shadow_range = self
            .frame_resources
            .shadow_frame
            .as_ref()
            .map_or(0.0, |s| s.uniforms.fade[1]);
        slots[SLOT_VOL as usize] = crate::atmosphere::volumetrics::uniforms(
            inv,
            r,
            shadow_range,
            half,
            self.look.volumetric_sky_share,
        );
        // The depth of field: focus on the camera target (camera-local 0).
        let view = glam::Mat4::from_cols_array_2d(&uniforms.view);
        let forward = glam::Vec3::new(view.x_axis.z, view.y_axis.z, view.z_axis.z);
        let eye = glam::Vec3::new(uniforms.eye[0], uniforms.eye[1], uniforms.eye[2]);
        let focal = (-eye).dot(forward).max(1.0);
        self.frame_resources.atmos.frame.focal = focal;
        self.frame_resources.atmos.dof_slots.clear();
        let mut slot = SLOT_DOF;
        let mut push = |slots: &mut Vec<PassUniforms>, iteration: f32, kind: f32| {
            slots[slot as usize] = dof_uniforms(inv, r, focal, forward.to_array(), iteration, kind);
            slot += 1;
            slot - 1
        };
        // Focus, two spreads, the far and near blurs, the composite.
        let focus = push(&mut slots, 0.0, 0.0);
        let mut dof = vec![(focus, 0.0)];
        for k in [0.0, 1.0] {
            dof.push((push(&mut slots, k, 0.0), k));
        }
        for kind in [0.0, 1.0] {
            for &k in &crate::post::dof::ITERATIONS {
                dof.push((push(&mut slots, k, kind), k));
            }
        }
        dof.push((push(&mut slots, 0.0, 0.0), 0.0));
        self.frame_resources.atmos.dof_slots = dof;
        self.bind_sky_cubes(device);
        let pipes = self.frame_resources.atmos.pipes.current().expect("pipes");
        // 256-byte slots.
        let mut bytes = vec![0_u8; (SLOT * u64::from(SLOTS)) as usize];
        for (k, s) in slots.iter().enumerate() {
            let b = bytemuck::bytes_of(s);
            bytes[k * SLOT as usize..k * SLOT as usize + b.len()].copy_from_slice(b);
        }
        queue.write_buffer(&pipes.slots, 0, &bytes);
        self.bind_atmos(device);
    }

    /// This frame's pass bind groups (the renderer's resolved and depth
    /// targets of this size).
    pub(crate) fn bind_atmos(&mut self, device: &wgpu::Device) {
        let f = self.frame_resources.atmos.frame;
        let (Some(t), Some(targets)) = (
            self.frame_resources.atmos.targets.as_ref(),
            self.frame_resources.targets.as_ref(),
        ) else {
            return;
        };
        let hdr = targets.hdr_views();
        let mut writes = 0;
        let sky = f
            .sky_shading
            .then(|| self.bind(device, [Some(&targets.scratch_view), None, None, None]))
            .flatten();
        let vol_key = t
            .vol
            .as_ref()
            .filter(|_| f.volumetrics)
            .map(|v| (targets.size, self.device_resources.samples, v.size));
        let vol = if vol_key.is_some() && vol_key == self.frame_resources.atmos.vol_bind_key {
            self.frame_resources.atmos.vol_bind.take()
        } else {
            t.vol.as_ref().filter(|_| f.volumetrics).and_then(|v| {
                Some([
                    self.bind(device, [None; 4])?,
                    self.bind(device, [None, Some(&v.depth), None, None])?,
                    self.bind(
                        device,
                        [Some(hdr[0]), Some(&v.depth), Some(&v.scatter), None],
                    )?,
                ])
            })
        };
        let dof_on = f.dof && t.dof.is_some();
        let vol_copy_back = vol.is_some() && dof_on;
        writes += usize::from(vol.is_some() && !vol_copy_back);
        // The depth of field reads the frame after the volumetrics.
        let src = hdr[writes % 2];
        let mut dof = Vec::new();
        if let Some(d) = t.dof.as_ref().filter(|_| f.dof) {
            let mut slots = self.frame_resources.atmos.dof_slots.iter().map(|&(s, _)| s);
            let mut add = |textures: [Option<&wgpu::TextureView>; 4]| {
                if let (Some(slot), Some(bind)) = (slots.next(), self.bind(device, textures)) {
                    dof.push((slot, bind));
                }
            };
            // Focus into focus[0]; the near weight spread 0 -> 1 -> 0.
            add([None; 4]);
            add([None, Some(&d.focus[0]), None, None]);
            add([None, Some(&d.focus[1]), None, None]);
            // The far and near blurs: Kawase chains from the frame,
            // ping-ponging in their pair (the last output lands in [n % 2]
            // of n - 1).
            let n = crate::post::dof::ITERATIONS.len();
            for chain in [&d.far, &d.near] {
                for k in 0..n {
                    let input = if k == 0 { src } else { &chain[(k - 1) % 2] };
                    add([Some(input), Some(&d.focus[0]), None, None]);
                }
            }
            let end = (n - 1) % 2;
            add([
                Some(src),
                Some(&d.focus[0]),
                Some(&d.far[end]),
                Some(&d.near[end]),
            ]);
        }
        writes += usize::from(dof.len() == 4 + 2 * crate::post::dof::ITERATIONS.len());
        self.frame_resources.atmos.sky_bind = sky;
        self.frame_resources.atmos.vol_bind_key = vol.as_ref().and(vol_key);
        self.frame_resources.atmos.vol_bind = vol;
        self.frame_resources.atmos.dof_binds = dof;
        self.frame_resources.atmos.hdr_writes = writes;
        self.frame_resources.atmos.vol_copy_back = vol_copy_back;
    }

    /// The frame's HDR resolve the post chain reads (`Targets::hdr_views`
    /// index): the one the atmosphere's last full-frame pass wrote.
    pub(crate) fn hdr_source(&self) -> usize {
        self.encoding_inputs().hdr_source()
    }

    pub(crate) fn bind<'a>(
        &'a self,
        device: &wgpu::Device,
        textures: [Option<&'a wgpu::TextureView>; 4],
    ) -> Option<wgpu::BindGroup> {
        let pipes = self.frame_resources.atmos.pipes.current()?;
        let depth = &self.frame_resources.targets.as_ref()?.depth;
        let t = |v: Option<&'a wgpu::TextureView>| v.unwrap_or(&pipes.dummy);
        let entry = |binding, view| wgpu::BindGroupEntry {
            binding,
            resource: wgpu::BindingResource::TextureView(view),
        };
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern atmosphere pass"),
            layout: &pipes.layout1,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &pipes.slots,
                        offset: 0,
                        size: wgpu::BufferSize::new(std::mem::size_of::<PassUniforms>() as u64),
                    }),
                },
                entry(1, t(textures[0])),
                entry(2, t(textures[1])),
                entry(3, t(textures[2])),
                entry(4, t(textures[3])),
                entry(5, depth),
            ],
        }))
    }

    /// The last frame's modern atmosphere layers (tests, logs).
    #[must_use]
    pub fn atmos_frame(&self) -> AtmosFrame {
        self.frame_resources.atmos.frame
    }
}

/// A pass's slot (`crate::atmosphere::sky::PassUniforms`): `p0` the depth-of-field
/// parameters, `p1` (focal point, iteration, pass kind, 0), `p2` the bokeh parameters, `p3` the
/// view's forward axis (camera-local, for the view depth).
pub(crate) fn dof_uniforms(
    inv_view_proj: [[f32; 4]; 4],
    rect: [f32; 4],
    focal: f32,
    forward: [f32; 3],
    iteration: f32,
    kind: f32,
) -> crate::atmosphere::sky::PassUniforms {
    crate::atmosphere::sky::PassUniforms {
        inv_view_proj,
        rect,
        p0: crate::post::dof::params(focal),
        p1: [focal, iteration, kind, 0.0],
        // No bokeh boost (smoothstep over an unreachable range).
        p2: [1.0e9, 2.0e9, 1.0, 1.0],
        p3: [forward[0], forward[1], forward[2], 0.0],
        ..Default::default()
    }
}

impl<'a> EncodeInputs<'a> {
    /// One fullscreen pass over the scene viewport, or over `half` texels
    /// from the origin (the volumetrics' half-size passes).
    pub(crate) fn atmos_pass(&self, encoder: &mut wgpu::CommandEncoder, draw: AtmosPass<'_>) {
        let AtmosPass {
            which,
            view,
            pipeline,
            bind,
            slot,
            shadowed,
            clear,
            half,
            cubes,
        } = draw;
        let (Some(pipes), Some(targets)) = (self.atmos.pipes.current(), self.targets.as_ref())
        else {
            return;
        };
        let (x, y, w, h, l, t, r, b) = match half {
            Some([hw, hh]) => (0.0, 0.0, hw as f32, hh as f32, 0, 0, hw, hh),
            None => self.pass_area(targets.size),
        };
        if r <= l || b <= t {
            return;
        }
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(which)),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: clear.map_or(wgpu::LoadOp::Load, |c| {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(c[0]),
                            g: f64::from(c[1]),
                            b: f64::from(c[2]),
                            a: 1.0,
                        })
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_scissor_rect(l, t, r - l, b - t);
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &pipes.frame_bind, &[]);
        pass.set_bind_group(1, bind, &[(u64::from(slot) * SLOT) as u32]);
        if shadowed {
            pass.set_bind_group(2, &self.shadow.atlas.2, &[]);
        }
        if let Some(cubes) = cubes {
            pass.set_bind_group(3, cubes, &[]);
        }
        pass.draw(0..3, 0..1);
    }

    /// The frame's HDR resolve the post chain reads (`Targets::hdr_views`
    /// index): the one the atmosphere's last full-frame pass wrote.
    pub(crate) fn hdr_source(&self) -> usize {
        self.atmos.hdr_writes % 2
    }

    /// Step 1b: the modern sky over the classic sky's target into the frame's
    /// colour target (`colour`).
    pub(crate) fn encode_sky_shading(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        colour: &wgpu::TextureView,
    ) {
        if !self.atmos.frame.sky_shading {
            return;
        }
        let (Some(pipes), Some(bind), Some(cubes)) = (
            self.atmos.pipes.current(),
            self.atmos.sky_bind.as_ref(),
            self.sky_cube_bind(),
        ) else {
            return;
        };
        // Cleared to the clear colour first (what the geometry is drawn over).
        self.atmos_pass(
            encoder,
            AtmosPass::new(
                crate::frame::passes::Pass::SkyShading,
                colour,
                &pipes.sky,
                bind,
                SLOT_SKY,
            )
            .with_cubes(cubes)
            .cleared(self.clear),
        );
    }

    /// The scene viewport and its clamped scissor of this frame.
    pub(crate) fn pass_area(&self, size: [u32; 2]) -> (f32, f32, f32, f32, u32, u32, u32, u32) {
        let [x, y, w, h] = self.atmos.rect;
        let [l, t, r, b] = self.atmos.clip;
        let (tw, th) = (size[0] as i32, size[1] as i32);
        let (l, t) = (l.clamp(0, tw), t.clamp(0, th));
        let (r, b) = (r.clamp(l, tw), b.clamp(t, th));
        (
            x as f32, y as f32, w as f32, h as f32, l as u32, t as u32, r as u32, b as u32,
        )
    }

    /// Step 3b (after the forward pass, before the tonemap): the volumetric
    /// scattering and the depth of field over the resolved HDR frame.
    pub(crate) fn encode_atmos_post(&self, encoder: &mut wgpu::CommandEncoder) {
        let f = self.atmos.frame;
        if !(f.volumetrics || f.dof) {
            return;
        }
        let (Some(pipes), Some(t), Some(targets)) = (
            self.atmos.pipes.current(),
            self.atmos.targets.as_ref(),
            self.targets.as_ref(),
        ) else {
            return;
        };
        // Each full-frame pass reads one of the frame's HDR resolves and
        // writes the other (`bind_atmos`).
        if let (Some(binds), Some(v)) = (
            self.atmos.vol_bind.as_ref().filter(|_| f.volumetrics),
            t.vol.as_ref(),
        ) {
            // The half-size path: the farthest depth of each 2x2, the
            // march over it, the bilateral apply at full size.
            self.atmos_pass(
                encoder,
                AtmosPass::new(
                    crate::frame::passes::Pass::VolumetricsDepth,
                    &v.depth,
                    &pipes.vol_depth,
                    &binds[0],
                    SLOT_VOL,
                )
                .over(v.size),
            );
            self.atmos_pass(
                encoder,
                AtmosPass::new(
                    crate::frame::passes::Pass::VolumetricsMarch,
                    &v.scatter,
                    &pipes.vol_march,
                    &binds[1],
                    SLOT_VOL,
                )
                .shadowed()
                .over(v.size),
            );
            self.atmos_pass(
                encoder,
                AtmosPass::new(
                    crate::frame::passes::Pass::Volumetrics,
                    &targets.scratch_view,
                    &pipes.vol_apply,
                    &binds[2],
                    SLOT_VOL,
                ),
            );
            if self.atmos.vol_copy_back {
                self.copy_back(encoder, &targets.scratch, &targets.resolved, targets.size);
            }
        }
        if let Some(d) = t.dof.as_ref().filter(|_| {
            f.dof && self.atmos.dof_binds.len() == 4 + 2 * crate::post::dof::ITERATIONS.len()
        }) {
            let n = crate::post::dof::ITERATIONS.len();
            let mut binds = self.atmos.dof_binds.iter();
            let mut next = |encoder: &mut wgpu::CommandEncoder,
                            which: crate::frame::passes::Pass,
                            out: &wgpu::TextureView,
                            pipeline: &wgpu::RenderPipeline| {
                if let Some((slot, bind)) = binds.next() {
                    self.atmos_pass(encoder, AtmosPass::new(which, out, pipeline, bind, *slot));
                }
            };
            next(
                encoder,
                crate::frame::passes::Pass::DofFocus,
                &d.focus[0],
                &pipes.dof_focus,
            );
            next(
                encoder,
                crate::frame::passes::Pass::DofSpread,
                &d.focus[1],
                &pipes.dof_spread,
            );
            next(
                encoder,
                crate::frame::passes::Pass::DofSpread,
                &d.focus[0],
                &pipes.dof_spread,
            );
            for chain in [&d.far, &d.near] {
                for k in 0..n {
                    next(
                        encoder,
                        crate::frame::passes::Pass::DofBlur,
                        &chain[k % 2],
                        &pipes.dof_blur,
                    );
                }
            }
            // The composite reads the frame's resolve (the volumetrics'
            // result copied back) and writes its twin.
            next(
                encoder,
                crate::frame::passes::Pass::DofComposite,
                &targets.scratch_view,
                &pipes.dof_composite,
            );
        }
    }

    pub(crate) fn copy_back(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        from: &wgpu::Texture,
        to: &wgpu::Texture,
        size: [u32; 2],
    ) {
        let (_, _, _, _, l, t, r, b) = self.pass_area(size);
        if r <= l || b <= t {
            return;
        }
        let at = wgpu::Origin3d { x: l, y: t, z: 0 };
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: from,
                mip_level: 0,
                origin: at,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: to,
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

    /// Step 1's target when the modern sky is on (`(attachment, resolve)`):
    /// the classic sky draws into the forward target and resolves into the
    /// frame's HDR twin, which the modern pass reads; at one sample it draws
    /// into the twin. (The modern pass clears the forward target before it
    /// draws, so the classic sky needs no target of its own:
    /// `frame::passes::ALIASES`.)
    pub(crate) fn sky_shading_target(
        &self,
    ) -> Option<(&wgpu::TextureView, Option<&wgpu::TextureView>)> {
        if !self.atmos.frame.sky_shading {
            return None;
        }
        let t = self.targets.as_ref()?;
        Some(match &t.msaa {
            Some(msaa) => (msaa, Some(&t.scratch_view)),
            None => (&t.scratch_view, None),
        })
    }
}

impl<'a> EncodeInputs<'a> {
    /// The atmosphere's pipelines at the current sample count (a new set).
    pub(crate) fn atmos_pipes(
        &self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) -> Pipes {
        let samples = self.samples;
        let module = self.shaders.get(
            device,
            crate::shaders::Module::Atmosphere {
                multisampled: samples > 1,
            },
        );
        Pipes::new(
            device,
            queue,
            samples,
            self.frame_buffer,
            &self.shadow.receive_layout,
            module,
        )
    }
}
