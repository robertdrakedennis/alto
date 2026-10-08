//! The point-light shadows' GPU side (the decisions and the evidence are
//! [`crate::shadows::point`] and [`crate::shadows::presets`]).
//!
//! - **Receive** ([`PointShadowMaps`], group 3 bindings 8-9 beside the
//!   point lights): the `PointLightShadows` block and the shared 2-D depth
//!   atlas. A slot owns a block of the atlas holding its six cube faces at
//!   every resolution level ([`AtlasLayout`]); the block names the current
//!   level's face rectangles, so the shader lookup is one rectangle per face.
//! - **Selection** ([`ModernRenderer::select_point_shadows`]): early in the
//!   frame, from the lights, the camera and the quality: the candidates, the
//!   slot pool, each slotted light's level. Point shadows exist exactly when
//!   the sun shadows do, at the sun shadows' quality.
//! - **Casters** ([`PointCasterGpu`]): `vs_point_shadow`/`fs_point_shadow`
//!   of the forward module into each face's rectangle, the depth `d² / r²`
//!   (linear in squared distance), the face camera's near 0.25 and far the
//!   radius. The casters are the frame's shadow-casting entity draws
//!   (visible and off-screen) whose posed bounds overlap the light's radius
//!   and the face clip volume (unknown bounds retain a conservative margin)
//!   (floors and the terrain do not cast: they are the receivers under a
//!   lamp). A face whose frustum does not meet the camera's is not drawn.
//! - **Reuse** (this crate's choice; unproven, see [`crate::shadows::point`]):
//!   a face of a slot's level is redrawn when its light changed (a new
//!   holder, it moved or its radius changed, the atlas or the scene changed),
//!   when its static casters' signature changed, or when dynamic casters meet
//!   it now or did when it was last drawn (the casters' classes of
//!   `shadows::cache`). What the face holds is then exactly what drawing it
//!   from scratch gives, so the frame is the frame of redrawing every face
//!   every frame. A still scene with an unchanged light set redraws nothing.

use crate::frame::gpu::shadow_cache::CasterClass;
use crate::frame::*;
use crate::lighting::point_lights::Light;
use crate::shadows::cache::Signature;
use crate::shadows::point::{
    candidates, level_for, AtlasLayout, LightInput, SlotPool, ViewFrustum, FACE_NEAR, MAX_LEVELS,
};
use crate::shadows::presets::{
    faces_met, point_preset, PointCasterUniforms, PointPreset, PointShadowUniforms, ShadowedLight,
    CASTER_SLOT, MAX_SHADOWED,
};

/// Preparation hint for deferred off-screen casters and unknown bounds:
/// three tiles in fine units. Posed face invalidation uses actual bounds.
pub(crate) const MARGIN: f32 = 1536.0;

/// Group 3's point-shadow entries (bindings 8-9).
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 2] {
    [
        wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 9,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ]
}

/// See the module docs ("Receive").
pub(crate) struct PointShadowMaps {
    pub(crate) uniforms: wgpu::Buffer,
    /// A block of no shadowed lights, for the passes that draw without
    /// point shadows (the captures).
    none: wgpu::Buffer,
    /// The atlas's view (sampled and rendered to).
    pub(crate) view: wgpu::TextureView,
    /// The atlas's size in texels.
    pub(crate) size: (u32, u32),
}

impl PointShadowMaps {
    /// No shadowed lights: a count of 0 and a one-texel atlas.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) -> Self {
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern point shadows"),
            size: std::mem::size_of::<PointShadowUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniforms,
            0,
            bytemuck::bytes_of(&PointShadowUniforms::default()),
        );
        let none = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern point shadows (none)"),
            contents: bytemuck::bytes_of(&PointShadowUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        Self {
            uniforms,
            none,
            view: atlas(device, 1, 1),
            size: (1, 1),
        }
    }

    pub(crate) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 2] {
        [
            wgpu::BindGroupEntry {
                binding: 8,
                resource: self.uniforms.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: wgpu::BindingResource::TextureView(&self.view),
            },
        ]
    }

    /// The entries with the block of no shadowed lights.
    pub(crate) fn entries_off(&self) -> [wgpu::BindGroupEntry<'_>; 2] {
        let [_, atlas] = self.entries();
        [
            wgpu::BindGroupEntry {
                binding: 8,
                resource: self.none.as_entire_binding(),
            },
            atlas,
        ]
    }

    /// Resize the atlas to `width` x `height` (true: recreated, so the bind
    /// group must be rebuilt and the atlas cleared).
    pub(crate) fn ensure(&mut self, device: &wgpu::Device, width: u32, height: u32) -> bool {
        if self.size == (width, height) {
            return false;
        }
        self.view = atlas(device, width, height);
        self.size = (width, height);
        true
    }
}

/// The depth atlas.
fn atlas(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("modern point shadow atlas"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: crate::shadows::SHADOW_FORMAT,
            // Tests read the atlas back.
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | if cfg!(test) {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

/// What a slot's drawn faces show: the holder (light id), its scene-local
/// position and radius bits, the face size and the scene.
pub(crate) type LayerKey = (
    usize,
    [u32; 3],
    u32,
    u32,
    crate::frame::gpu::point_lights::GridKey,
);

/// What a face holds: its static casters' signature and whether dynamic
/// casters were drawn.
type FaceState = (Signature, bool);

/// A slotted light that casts this frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ActiveLight {
    pub(crate) slot: usize,
    /// Index into the frame's lights.
    pub(crate) light: usize,
    /// The resolution level its faces are drawn and sampled at.
    pub(crate) level: usize,
}

/// One slot's work this frame.
pub(crate) struct PendingSlot {
    pub(crate) slot: usize,
    pub(crate) level: usize,
    /// Clear the slot's whole block first (a new holder or light).
    pub(crate) clear_block: bool,
    /// The faces redrawn (bit `f`) and every visible face's casters.
    pub(crate) redraw: u8,
    pub(crate) faces: [Vec<CasterRef>; 6],
}

/// This frame's point-shadow work.
#[derive(Default)]
pub(crate) struct PointPlan {
    /// The atlas was just created: clear all of it.
    pub(crate) clear_atlas: bool,
    pub(crate) slots: Vec<PendingSlot>,
}

#[derive(Clone, Copy)]
pub(crate) enum CasterRef {
    Draw(usize),
    ShadowOnly(usize),
}

/// See the module docs ("Casters", "Reuse").
pub(crate) struct PointCasterGpu {
    pub(crate) pipeline: wgpu::RenderPipeline,
    pub(crate) casters: wgpu::Buffer,
    pub(crate) caster_bind: wgpu::BindGroup,
    /// The quality's preset the pool and the atlas were made for, and the
    /// scene whose lights the pool's ids name.
    pub(crate) preset: Option<PointPreset>,
    pub(crate) scene: Option<crate::frame::gpu::point_lights::GridKey>,
    pub(crate) pool: SlotPool,
    pub(crate) layout: Option<AtlasLayout>,
    /// The camera's frustum this frame (set by the selection).
    pub(crate) frustum: Option<ViewFrustum>,
    /// This frame's slotted candidates.
    pub(crate) active: Vec<ActiveLight>,
    /// Each slot's drawn light, and what each face of each level holds.
    pub(crate) keys: [Option<LayerKey>; MAX_SHADOWED],
    pub(crate) faces: [[[Option<FaceState>; 6]; MAX_LEVELS]; MAX_SHADOWED],
    pub(crate) plan: PointPlan,
    /// Whether the block holds shadowed lights (a count of 0 is written
    /// once when they go).
    pub(crate) lit: bool,
    /// Tests: no point shadows (the frame without them).
    #[cfg(test)]
    pub(crate) test_off: bool,
    /// Tests: every off-screen static caster is prepared for the faces (the
    /// frame the reach test must equal).
    #[cfg(test)]
    pub(crate) test_all_casters: bool,
}

/// The cache and slot selection before an unsent frame was prepared.
pub(crate) struct PointProgress {
    preset: Option<PointPreset>,
    scene: Option<crate::frame::gpu::point_lights::GridKey>,
    pool: SlotPool,
    keys: [Option<LayerKey>; MAX_SHADOWED],
    faces: [[[Option<FaceState>; CUBE_FACES]; MAX_LEVELS]; MAX_SHADOWED],
    atlas_size: (u32, u32),
}

const CUBE_FACES: usize = 6;

impl PointCasterGpu {
    pub(crate) fn checkpoint(&self, atlas_size: (u32, u32)) -> PointProgress {
        PointProgress {
            preset: self.preset,
            scene: self.scene,
            pool: self.pool.clone(),
            keys: self.keys,
            faces: self.faces,
            atlas_size,
        }
    }

    pub(crate) fn restore(&mut self, previous: PointProgress, atlas_size: (u32, u32)) {
        self.preset = previous.preset;
        self.scene = previous.scene;
        self.pool = previous.pool;
        self.keys = previous.keys;
        self.faces = previous.faces;
        if atlas_size != previous.atlas_size {
            self.forget_faces();
        }
        self.plan.slots.clear();
        self.plan.clear_atlas = false;
    }
    pub(crate) fn new(
        device: &wgpu::Device,
        module: &wgpu::ShaderModule,
        frame_layout: &wgpu::BindGroupLayout,
        material_layout: &wgpu::BindGroupLayout,
        arrays_layout: &wgpu::BindGroupLayout,
        vertex_layouts: &[Option<wgpu::VertexBufferLayout<'_>>],
    ) -> Self {
        let caster_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern point shadow caster"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 4,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(
                        std::mem::size_of::<PointCasterUniforms>() as u64,
                    ),
                },
                count: None,
            }],
        });
        let casters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern point shadow casters"),
            size: CASTER_SLOT * (6 * MAX_SHADOWED) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let caster_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern point shadow caster"),
            layout: &caster_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &casters,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<PointCasterUniforms>() as u64),
                }),
            }],
        });
        let pipeline = {
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("modern point shadow caster"),
                bind_group_layouts: &[
                    Some(frame_layout),
                    Some(material_layout),
                    Some(&caster_layout),
                    None,
                    Some(arrays_layout),
                ],
                immediate_size: 0,
            });
            // Two-sided like the sun's casters (the classic models are often open
            // meshes); the bias is the receiver's.
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("modern point shadow caster"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_point_shadow"),
                    buffers: vertex_layouts,
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_point_shadow"),
                    targets: &[],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: crate::shadows::SHADOW_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        Self {
            pipeline,
            casters,
            caster_bind,
            preset: None,
            scene: None,
            pool: SlotPool::default(),
            layout: None,
            frustum: None,
            active: Vec::new(),
            keys: [None; MAX_SHADOWED],
            faces: [[[None; 6]; MAX_LEVELS]; MAX_SHADOWED],
            plan: PointPlan::default(),
            lit: false,
            #[cfg(test)]
            test_off: false,
            #[cfg(test)]
            test_all_casters: false,
        }
    }

    /// Forget what every face holds (every visible face is redrawn next).
    pub(crate) fn forget_faces(&mut self) {
        self.keys = [None; MAX_SHADOWED];
        self.faces = [[[None; 6]; MAX_LEVELS]; MAX_SHADOWED];
    }

    /// Forget the slots and what they hold.
    fn forget_slots(&mut self) {
        self.pool.clear();
        self.forget_faces();
    }

    /// Whether a point at camera-local `p` is within reach of a shadowed
    /// light's casters this frame (a caster's position, for the off-screen
    /// casters that are only prepared when a point shadow may need them).
    pub(crate) fn reaches(&self, lights: &[Light], origin: [f32; 3], p: [f32; 3]) -> bool {
        #[cfg(test)]
        if self.test_all_casters {
            return true;
        }
        self.active.iter().any(|a| {
            let l = &lights[a.light];
            let d = [
                p[0] - (l.pos[0] - origin[0]),
                p[1] - (l.pos[1] - origin[1]),
                p[2] - (l.pos[2] - origin[2]),
            ];
            let reach = l.radius + MARGIN;
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= reach * reach
        })
    }
}

impl ModernRenderer {
    /// This frame's point-light shadow selection (see the module docs): the
    /// quality's preset (the sun shadows'; none without them), the
    /// candidates, the slot pool and each slotted light's level. Runs once the
    /// frame's lights and camera are known, before the casters are planned.
    pub(crate) fn select_point_shadows(
        &mut self,
        view: &[[f32; 4]; 4],
        view_proj: &[[f32; 4]; 4],
        eye: [f32; 3],
        origin: [f32; 3],
    ) {
        let scene = self.lights.key();
        let point = &mut self.shadow.point;
        point.active.clear();
        let preset = self
            .shadow_frame
            .as_ref()
            .map(|f| point_preset(f.profile.quality));
        #[cfg(test)]
        let preset = preset.filter(|_| !point.test_off);
        let Some(preset) = preset else {
            point.frustum = None;
            return;
        };
        if point.preset.map(|p| p.level) != Some(preset.level) || point.scene != Some(scene) {
            point.forget_slots();
            point.preset = Some(preset);
            point.scene = Some(scene);
        }
        let frustum = ViewFrustum::new(view, view_proj, eye);
        let inputs: Vec<LightInput> = self
            .lights
            .frame
            .iter()
            .map(|l| LightInput {
                pos: [
                    l.pos[0] - origin[0],
                    l.pos[1] - origin[1],
                    l.pos[2] - origin[2],
                ],
                radius: l.radius,
                intensity: l.intensity,
                casts: l.casts_shadows,
            })
            .collect();
        let candidates = candidates(&inputs, &frustum, preset.max_view_distance);
        let slots = point
            .pool
            .update(preset.lights.min(MAX_SHADOWED), &candidates);
        point.active = slots
            .iter()
            .map(|a| ActiveLight {
                slot: a.slot,
                light: a.light,
                level: level_for(&inputs[a.light], &frustum, preset.levels),
            })
            .collect();
        point.frustum = Some(frustum);
    }

    /// This frame's point-light shadows (see the module docs): the atlas and
    /// the block for the slotted lights ([`Self::select_point_shadows`]), and
    /// the faces to draw with their casters. Runs after the frame's draws are
    /// recorded.
    pub(crate) fn prepare_point_shadows(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        origin: [f32; 3],
    ) {
        self.shadow.point.plan.slots.clear();
        self.shadow.point.plan.clear_atlas = false;
        let (Some(preset), Some(frustum)) =
            (self.shadow.point.preset, self.shadow.point.frustum.clone())
        else {
            self.shadow.point.active.clear();
            self.write_point_block(queue, None, &[]);
            return;
        };
        let active = self.shadow.point.active.clone();
        if active.is_empty() {
            self.write_point_block(queue, Some(&preset), &[]);
            return;
        }
        let layout = AtlasLayout::new(preset.face, preset.levels, preset.lights);
        self.shadow.point.layout = Some(layout);
        if self
            .lights
            .shadow_maps
            .ensure(device, layout.width, layout.height)
        {
            self.lights.rebind(device);
            self.shadow.point.forget_faces();
            self.shadow.point.plan.clear_atlas = true;
        }
        let scene = self.lights.key();
        // The block: the slotted lights in slot order, each at its level.
        let shadowed: Vec<ShadowedLight> = active
            .iter()
            .map(|a| {
                let (scale, faces) = layout.uv(a.slot, a.level);
                ShadowedLight {
                    light_slot: a.light as u32 + 1,
                    level: a.level,
                    scale,
                    faces,
                }
            })
            .collect();
        self.write_point_block(queue, Some(&preset), &shadowed);
        let fresh = self.shadow.point.plan.clear_atlas;
        // Each slot's faces: a changed light redraws them all, else the
        // visible faces whose casters changed (see the module docs).
        let mut uniforms = [PointCasterUniforms::default(); 6 * MAX_SHADOWED];
        for a in &active {
            let l = self.lights.frame[a.light];
            let key: LayerKey = (
                a.light,
                l.pos.map(f32::to_bits),
                l.radius.to_bits(),
                preset.face,
                scene,
            );
            let mut clear_block = fresh;
            if self.shadow.point.keys[a.slot] != Some(key) {
                self.shadow.point.keys[a.slot] = Some(key);
                self.shadow.point.faces[a.slot] = [[None; 6]; MAX_LEVELS];
                clear_block = true;
            }
            let p = [
                l.pos[0] - origin[0],
                l.pos[1] - origin[1],
                l.pos[2] - origin[2],
            ];
            let r = l.radius;
            for (f, u) in uniforms[a.slot * 6..a.slot * 6 + 6].iter_mut().enumerate() {
                *u = PointCasterUniforms {
                    light: [p[0], p[1], p[2], 1.0 / (r * r)],
                    face: [f as f32, FACE_NEAR, r, 0.0],
                    ..PointCasterUniforms::default()
                };
            }
            // The faces the camera can see through (the others keep what
            // they hold).
            let visible = (0..6).fold(0_u8, |m, f| m | u8::from(frustum.meets_face(p, f, r)) << f);
            let mut faces: [Vec<CasterRef>; 6] = Default::default();
            let mut state = [(Signature::default(), false); 6];
            let mut add = |at: [f32; 3],
                           bounds: Option<crate::models::bounds::Bounds>,
                           caster: CasterRef,
                           class: CasterClass| {
                let d = [at[0] - p[0], at[1] - p[1], at[2] - p[2]];
                let mask = match bounds {
                    Some(bounds) => overlapping_faces(&bounds, p, r),
                    None => {
                        if (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() > r + MARGIN {
                            return;
                        }
                        faces_met(d, MARGIN)
                    }
                } & visible;
                for (f, list) in faces.iter_mut().enumerate() {
                    if mask & (1 << f) != 0 {
                        list.push(caster);
                        match class {
                            CasterClass::Static(h) => state[f].0.add(h),
                            CasterClass::Dynamic => state[f].1 = true,
                            // Counted with its entity's first draw (the
                            // same position, so the same faces).
                            CasterClass::StaticPart | CasterClass::None => {}
                        }
                    }
                }
            };
            let at = |instance: u32| {
                let m = &self.instances[instance as usize].model;
                [m[12], m[13], m[14]]
            };
            let classes = &self.shadow.sun;
            let mut ranges = self.draw_bounds.iter().peekable();
            for (k, d) in self.draws.iter().enumerate() {
                while ranges.next_if(|r| r.1 <= k as u32).is_some() {}
                let bounds = ranges.peek().filter(|r| r.0 <= k as u32).map(|r| r.2);
                if d.casts && !matches!(d.geometry, Geometry::Floor { .. }) {
                    let class = classes.draw_classes.get(k).copied();
                    add(
                        at(d.instance),
                        bounds,
                        CasterRef::Draw(k),
                        class.unwrap_or(CasterClass::Dynamic),
                    );
                }
            }
            for (k, (d, _)) in self.shadow_only.iter().enumerate() {
                let class = classes.only_classes.get(k).copied();
                add(
                    at(d.instance),
                    self.shadow_only_bounds.get(k).copied().flatten(),
                    CasterRef::ShadowOnly(k),
                    class.unwrap_or(CasterClass::Dynamic),
                );
            }
            let mut redraw = 0_u8;
            for (f, now) in state.iter().enumerate() {
                if visible & (1 << f) == 0 {
                    continue;
                }
                let held = &mut self.shadow.point.faces[a.slot][a.level][f];
                if now.1 || held.is_none_or(|h| h != *now || h.1) {
                    redraw |= 1 << f;
                    *held = Some(*now);
                    self.stats.point_shadow_draws += faces[f].len();
                }
            }
            if clear_block || redraw != 0 {
                self.shadow.point.plan.slots.push(PendingSlot {
                    slot: a.slot,
                    level: a.level,
                    clear_block,
                    redraw,
                    faces,
                });
            }
        }
        self.stats.point_shadow_lights = active.len();
        queue.write_buffer(
            &self.shadow.point.casters,
            0,
            bytemuck::cast_slice(&uniforms),
        );
    }

    /// Write the shader block: the shadowed lights, or (once) none.
    fn write_point_block(
        &mut self,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        preset: Option<&PointPreset>,
        lights: &[ShadowedLight],
    ) {
        let lit = !lights.is_empty();
        if !lit && !self.shadow.point.lit {
            return;
        }
        let block = match preset {
            Some(preset) if lit => PointShadowUniforms::new(preset, lights),
            _ => PointShadowUniforms::default(),
        };
        queue.write_buffer(
            &self.lights.shadow_maps.uniforms,
            0,
            bytemuck::bytes_of(&block),
        );
        self.shadow.point.lit = lit;
    }

    /// Whether the point shadows record a pass this frame.
    pub(crate) fn point_shadows_record(&self) -> bool {
        let plan = &self.shadow.point.plan;
        plan.clear_atlas || !plan.slots.is_empty()
    }

    /// The point-shadow faces this frame redraws: one pass over the atlas,
    /// each pending slot's block cleared if its light changed, then each
    /// redrawn face cleared and drawn into its rectangle.
    pub(crate) fn encode_point_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        let point = &self.shadow.point;
        let Some(layout) = point.layout else {
            return;
        };
        let plan = &point.plan;
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::PointShadows)),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.lights.shadow_maps.view,
                depth_ops: Some(wgpu::Operations {
                    load: if plan.clear_atlas {
                        wgpu::LoadOp::Clear(1.0)
                    } else {
                        wgpu::LoadOp::Load
                    },
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            occlusion_query_set: None,
            multiview_mask: None,
            timestamp_writes: None,
        });
        let fill = &self.shadow.sun.fill;
        for pending in &plan.slots {
            if pending.clear_block && !plan.clear_atlas {
                let [x, y, w, h] = layout.block(pending.slot);
                pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0.0, 1.0);
                pass.set_scissor_rect(x, y, w, h);
                fill.clear(&mut pass);
            }
            for (f, list) in pending.faces.iter().enumerate() {
                if pending.redraw & (1 << f) == 0 {
                    continue;
                }
                let r = layout.rect(pending.slot, pending.level, f);
                pass.set_viewport(
                    r.x as f32,
                    r.y as f32,
                    r.size as f32,
                    r.size as f32,
                    0.0,
                    1.0,
                );
                pass.set_scissor_rect(r.x, r.y, r.size, r.size);
                if !pending.clear_block && !plan.clear_atlas {
                    fill.clear(&mut pass);
                }
                pass.set_pipeline(&point.pipeline);
                pass.set_bind_group(0, &self.frame_bind, &[]);
                let offset = (pending.slot * 6 + f) as u64 * CASTER_SLOT;
                pass.set_bind_group(2, &point.caster_bind, &[offset as u32]);
                let mut bound = crate::frame::submit::Bound::default();
                for caster in list {
                    let d = match *caster {
                        CasterRef::Draw(k) => &self.draws[k],
                        CasterRef::ShadowOnly(k) => &self.shadow_only[k].0,
                    };
                    self.submit(&mut pass, d, &mut bound);
                }
            }
        }
    }
}

/// Faces a posed box can actually write. The radius test matches the radial
/// depth output; the clip planes match the caster shader's cube projection.
fn overlapping_faces(bounds: &crate::models::bounds::Bounds, light: [f32; 3], radius: f32) -> u8 {
    let p = glam::Vec3::from(light);
    let lo = glam::Vec3::from(bounds.min);
    let hi = glam::Vec3::from(bounds.max);
    const RELATIVE_OVERLAP_TOLERANCE: f32 = 1e-4;
    const ABSOLUTE_OVERLAP_TOLERANCE: f32 = 1e-4;
    let epsilon = radius.abs() * RELATIVE_OVERLAP_TOLERANCE + ABSOLUTE_OVERLAP_TOLERANCE;
    if p.distance_squared(p.clamp(lo, hi)) > (radius + epsilon).powi(2) {
        return 0;
    }
    (0..CUBE_FACES).fold(0, |mask, face| {
        let (axis, u, v) = crate::shadows::point::face_axes(face);
        let (axis, u, v) = (axis.as_vec3(), u.as_vec3(), v.as_vec3());
        let z = radius / (radius - FACE_NEAR);
        let rows = [
            u.extend(-u.dot(p)),
            v.extend(-v.dot(p)),
            (axis * z).extend(-z * (axis.dot(p) + FACE_NEAR)),
            axis.extend(-axis.dot(p)),
        ];
        let clip = glam::Mat4::from_cols(rows[0], rows[1], rows[2], rows[3]).transpose();
        mask | (u8::from(!bounds.outside(&clip)) << face)
    })
}

#[cfg(test)]
mod overlap_tests {
    use super::*;
    use crate::models::bounds::Bounds;

    #[test]
    fn posed_caster_bounds_reject_margin_only_overlap_and_keep_face_crossings() {
        const RADIUS: f32 = 100.0;
        const OUTSIDE_BOX: Bounds = Bounds {
            min: [120.0, -2.0, -2.0],
            max: [124.0, 2.0, 2.0],
        };
        const IN_FACE: Bounds = Bounds {
            min: [20.0, -2.0, -2.0],
            max: [24.0, 2.0, 2.0],
        };
        const EDGE: Bounds = Bounds {
            min: [20.0, 19.0, -2.0],
            max: [24.0, 25.0, 2.0],
        };
        const HUGE: Bounds = Bounds {
            min: [-120.0; 3],
            max: [120.0; 3],
        };
        const POSITIVE_X: u8 = 1;
        const POSITIVE_Y: u8 = 1 << 2;
        assert_eq!(overlapping_faces(&OUTSIDE_BOX, [0.0; 3], RADIUS), 0);
        assert_eq!(overlapping_faces(&IN_FACE, [0.0; 3], RADIUS), POSITIVE_X);
        assert_eq!(
            overlapping_faces(&EDGE, [0.0; 3], RADIUS),
            POSITIVE_X | POSITIVE_Y
        );
        assert_eq!(
            overlapping_faces(&HUGE, [0.0; 3], RADIUS),
            (1 << CUBE_FACES) - 1
        );
        let shift = glam::Mat4::from_translation(glam::Vec3::splat(RADIUS)).to_cols_array();
        assert_eq!(
            overlapping_faces(&IN_FACE.transformed(&shift), [RADIUS; 3], RADIUS),
            POSITIVE_X
        );
    }
}
