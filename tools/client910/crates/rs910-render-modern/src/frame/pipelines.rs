//! The renderer's pipelines: one cache ([`Variants`]) keyed by module
//! variant and the forward target's sample count (MSAA follows the client's
//! anti-aliasing level). Every set the frame draws with is created with the
//! renderer (`ModernRenderer::new`: the forward-target set here, the probe
//! capture, terrain, water and atmosphere sets, the post chain); an
//! anti-aliasing change builds only the sets that depend on the count, once
//! per count, and changing back compiles nothing. Everything else the
//! renderer holds (materials, loc meshes, floors, terrain, far scene,
//! probes, shadows) does not depend on the count and stays; the targets
//! follow the count on the next frame (`ensure_targets`, the atmosphere's
//! by key).

use std::hash::Hash;

use crate::frame::compile::Job;
use crate::frame::*;

/// The renderer's pipeline cache: each pipeline set built once per key (its
/// module variant and the forward target's sample count) and kept, so an
/// anti-aliasing change back compiles nothing. The forward-target set, the
/// terrain's lit pass, the water's and the atmosphere's sets are held this
/// way; everything that does not depend on the key is built once beside it.
pub(crate) struct Variants<K, T> {
    built: HashMap<K, T>,
    current: Option<K>,
}

impl<K, T> Default for Variants<K, T> {
    fn default() -> Self {
        Self {
            built: HashMap::new(),
            current: None,
        }
    }
}

impl<K: Copy + Eq + Hash, T> Variants<K, T> {
    /// Draw with `key`'s set from now on, built by `build` the first time.
    pub(crate) fn select(&mut self, key: K, build: impl FnOnce() -> T) -> &mut T {
        self.current = Some(key);
        self.built.entry(key).or_insert_with(build)
    }

    /// Whether `key`'s set is built.
    pub(crate) fn contains(&self, key: &K) -> bool {
        self.built.contains_key(key)
    }

    /// Keep `set` as `key`'s (built elsewhere, at the same time as other
    /// sets: `frame::compile`) and draw with it from now on.
    pub(crate) fn insert_selected(&mut self, key: K, set: T) {
        self.built.entry(key).or_insert(set);
        self.current = Some(key);
    }

    /// Draw with `key`'s set (built) from now on.
    pub(crate) fn use_key(&mut self, key: K) {
        assert!(self.contains(&key), "the set is built before it is used");
        self.current = Some(key);
    }

    /// The selected set (`None`: nothing selected yet).
    pub(crate) fn current(&self) -> Option<&T> {
        self.current.and_then(|k| self.built.get(&k))
    }
}

/// What the forward-target pipelines are built from, kept for a sample
/// change: the forward module (its WGSL composed and compiled once, shared
/// by the forward, sprite, shadow, water reflection and probe capture
/// pipelines), the forward pipeline layout, the sky layer module and
/// layout.
pub(crate) struct PipelineInputs {
    pub(crate) forward_module: std::sync::Arc<wgpu::ShaderModule>,
    pub(crate) forward_layout: wgpu::PipelineLayout,
    pub(crate) sky_module: std::sync::Arc<wgpu::ShaderModule>,
    pub(crate) sky_layout: wgpu::PipelineLayout,
}

/// The renderer's own pipelines that draw into the forward target.
pub(crate) struct SamplePipelines {
    pub(crate) forward: wgpu::RenderPipeline,
    pub(crate) forward_no_depth_write: wgpu::RenderPipeline,
    pub(crate) depth_prepass: wgpu::RenderPipeline,
    pub(crate) sky_layer: wgpu::RenderPipeline,
    pub(crate) sprites: [wgpu::RenderPipeline; 2],
}

impl SamplePipelines {
    pub(crate) fn new(device: &wgpu::Device, inputs: &PipelineInputs, samples: u32) -> Self {
        let multisample = wgpu::MultisampleState {
            count: samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };
        let primitive = wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleList,
            front_face: wgpu::FrontFace::Ccw,
            cull_mode: Some(wgpu::Face::Back),
            ..Default::default()
        };
        let layouts = vertex_layouts();
        let module = &inputs.forward_module;
        let model_pipeline = |label: &str,
                              depth: Option<(bool, wgpu::CompareFunction)>,
                              colour: bool,
                              (vs, fs): (&str, &str)| {
            let targets = [Some(wgpu::ColorTargetState {
                format: HDR_FORMAT,
                blend: Some(ALPHA_BLEND),
                write_mask: wgpu::ColorWrites::ALL,
            })];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&inputs.forward_layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some(vs),
                    buffers: &layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some(fs),
                    targets: if colour { &targets } else { &[] },
                    compilation_options: Default::default(),
                }),
                primitive,
                depth_stencil: depth.map(|(write, compare)| wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample,
                multiview_mask: None,
                cache: None,
            })
        };
        // The lit models and floors (the probe ambient), the depth
        // pre-pass (its coverage test only), the sky layer and the sprites: created at once
        // (`frame::compile`).
        let sky_layer = || {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern sky layer"),
                layout: Some(&inputs.sky_layout),
                vertex: wgpu::VertexState {
                    module: &inputs.sky_module,
                    entry_point: Some("vs_main"),
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &inputs.sky_module,
                    entry_point: Some("fs_main"),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: Some(ALPHA_BLEND),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample,
                multiview_mask: None,
                cache: None,
            })
        };
        let jobs: Vec<Job<'_, wgpu::RenderPipeline>> = vec![
            Box::new(|| {
                model_pipeline(
                    "modern forward",
                    Some((true, wgpu::CompareFunction::LessEqual)),
                    true,
                    ("vs_forward", "fs_forward"),
                )
            }),
            Box::new(|| {
                model_pipeline(
                    "modern forward (no depth write)",
                    Some((false, wgpu::CompareFunction::LessEqual)),
                    true,
                    ("vs_forward", "fs_forward"),
                )
            }),
            Box::new(|| {
                model_pipeline(
                    "modern depth pre-pass",
                    Some((true, wgpu::CompareFunction::LessEqual)),
                    false,
                    ("vs_main", "fs_depth"),
                )
            }),
            Box::new(sky_layer),
            Box::new(|| SpriteGpu::pipeline(device, module, &inputs.forward_layout, samples, true)),
            Box::new(|| {
                SpriteGpu::pipeline(device, module, &inputs.forward_layout, samples, false)
            }),
        ];
        let mut built = crate::frame::compile::all(jobs).into_iter();
        let mut next = || built.next().expect("every pipeline is built");
        Self {
            forward: next(),
            forward_no_depth_write: next(),
            depth_prepass: next(),
            sky_layer: next(),
            sprites: [next(), next()],
        }
    }
}

impl ModernRenderer {
    /// Draw the next frames into a forward target of `samples` (1 or a
    /// count the device supports for [`HDR_FORMAT`] with [`DEPTH_FORMAT`];
    /// the shell follows the client's anti-aliasing level). Only what
    /// depends on the count is rebuilt (see the module docs); a frame drawn
    /// after the change is the frame a renderer created with `samples`
    /// draws, once its probes and caches have settled.
    pub fn set_samples(&mut self, device: &wgpu::Device, samples: u32) {
        if samples == self.samples {
            return;
        }
        let inputs = &self.pipeline_inputs;
        self.pipelines
            .select(samples, || SamplePipelines::new(device, inputs, samples));
        log::info!(
            "[modern] forward target: {} -> {samples} samples",
            self.samples
        );
        self.samples = samples;
    }

    /// Build every pipeline set that depends on the sample count for each
    /// count of `counts` (the forward-target set, the terrain's lit pass, the
    /// water's and the atmosphere's sets), as a frame at that count would,
    /// and keep drawing at the current count: a later [`Self::set_samples`]
    /// to one of them compiles nothing (performance plan P5; the startup
    /// thread builds the counts the client may switch to,
    /// `frame::startup`).
    pub fn prepare_sample_counts(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        counts: &[u32],
    ) {
        let current = self.samples;
        for &samples in counts {
            self.set_samples(device, samples);
            self.select_count_sets(device, queue);
        }
        self.set_samples(device, current);
        self.select_count_sets(device, queue);
    }

    /// The pipelines the renderer needs at the current sample count that
    /// are not built yet, created at once (`frame::compile`): the terrain's,
    /// the water's and the atmosphere's sets (`new` and each count of
    /// [`Self::prepare_sample_counts`]), and, the first time, the probe
    /// capture's and the caustic buffers'.
    pub(crate) fn select_count_sets(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) {
        let samples = self.samples;
        let water_debug = match crate::modern_debug_flags::flags().water {
            Some(crate::modern_debug_flags::WaterDebug::Term(n)) => n,
            _ => 0,
        };
        let need = (
            self.probes.pipes.is_none(),
            self.water.caustics.buffers.is_none(),
            self.terrain.pipes.is_none(),
            !self.water.pipes.contains(&(samples, water_debug)),
            !self.atmos.pipes.contains(&samples),
        );
        let built = {
            let this = &*self;
            let ((probes, caustics), ((terrain, water), atmos)) = crate::frame::compile::join(
                || {
                    crate::frame::compile::join(
                        || {
                            need.0.then(|| {
                                crate::frame::gpu::probes::CapturePipes::new(device, queue, this)
                            })
                        },
                        || need.1.then(|| this.caustic_buffers(device)),
                    )
                },
                || {
                    crate::frame::compile::join(
                        || {
                            crate::frame::compile::join(
                                || need.2.then(|| this.terrain_pipes(device)),
                                || need.3.then(|| this.water_pipes(device, queue, water_debug)),
                            )
                        },
                        || need.4.then(|| this.atmos_pipes(device, queue)),
                    )
                },
            );
            (probes, caustics, terrain, water, atmos)
        };
        let (probes, caustics, terrain, water, atmos) = built;
        if let Some(probes) = probes {
            self.probes.pipes = Some(probes);
        }
        if let Some(caustics) = caustics {
            self.water.caustics.buffers = Some(caustics);
        }
        if let Some(terrain) = terrain {
            self.terrain.pipes = Some(terrain);
        }
        if let Some(water) = water {
            self.water
                .pipes
                .insert_selected((samples, water_debug), water);
        }
        if let Some(atmos) = atmos {
            self.atmos.pipes.insert_selected(samples, atmos);
        }
        // What was built before (another count's set, this count's lit
        // terrain pass) is selected here.
        self.select_terrain_pipes(device);
        self.select_water_pipes(device, queue, water_debug);
        self.select_atmos_pipes(device, queue);
    }

    /// The forward-target pipelines at the current sample count.
    pub(crate) fn pipes(&self) -> &SamplePipelines {
        self.pipelines
            .current()
            .expect("the forward-target pipelines")
    }
}
