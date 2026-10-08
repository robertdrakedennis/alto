//! The shadow caches' GPU half (lane P3-SHADOWS; the CPU half and the
//! reasons are [`crate::shadows::cache`]): the casters' classes and
//! signatures, each sun cascade's plan and caster lists, the static atlas,
//! the tile fills (clear and restore) and the sun shadow passes.
use crate::frame::encoding::EncodeInputs;
use crate::frame::*;
use crate::shadows::cache::{draw_hash, selection_word, Mix, Signature, SunCache, TilePlan};
use crate::shadows::MAX_CASCADES;

/// A caster draw's class this frame (`shadows::cache`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CasterClass {
    /// Not drawn into the shadow maps.
    None,
    /// Static, with its hash.
    Static(u64),
    /// Static, counted in the hash of its entity's first draw (an
    /// off-screen caster's further draws).
    StaticPart,
    Dynamic,
}

/// What the cache remembers of an off-screen caster it prepared: the model
/// it was prepared for, the shadow settings, its casting draws and whether
/// one of them scrolls (a dynamic caster).
#[derive(Clone, Copy, Debug)]
pub(crate) struct PreparedCaster {
    pub(crate) key: EntityKey,
    pub(crate) fingerprint: (usize, i32, i32),
    pub(crate) settings: (bool, bool),
    pub(crate) draws: usize,
    pub(crate) scrolls: bool,
}

/// An off-screen caster: its entity, cascades and hash (`None`: a dynamic
/// caster), and its casting draws when known. A static one whose draws a
/// frame may skip is deferred.
pub(crate) struct OffScreenCaster<'a> {
    pub(crate) entity: crate::models::draw_list::EntityDraw<'a>,
    pub(crate) mask: u8,
    pub(crate) hash: Option<u64>,
    pub(crate) draws: usize,
}

/// The tile fills: a clear (depth 1) and a restore from the static atlas.
pub(crate) struct FillGpu {
    pub(crate) clear: wgpu::RenderPipeline,
    pub(crate) restore: wgpu::RenderPipeline,
    pub(crate) restore_layout: wgpu::BindGroupLayout,
}

impl FillGpu {
    pub(crate) fn new(device: &wgpu::Device, module: &wgpu::ShaderModule) -> Self {
        let restore_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern shadow restore"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Depth,
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline =
            |label, layouts: &[Option<&wgpu::BindGroupLayout>], fragment: Option<&str>| {
                let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some(label),
                    bind_group_layouts: layouts,
                    immediate_size: 0,
                });
                device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some(label),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_fill"),
                        buffers: &[],
                        compilation_options: Default::default(),
                    },
                    fragment: fragment.map(|entry_point| wgpu::FragmentState {
                        module,
                        entry_point: Some(entry_point),
                        targets: &[],
                        compilation_options: Default::default(),
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: crate::shadows::SHADOW_FORMAT,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: Default::default(),
                        bias: Default::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                })
            };
        let pipeline = &pipeline;
        let restore_layouts = [Some(&restore_layout)];
        let (clear, restore) = crate::frame::compile::join(
            move || pipeline("modern shadow tile clear", &[], None),
            || {
                pipeline(
                    "modern shadow tile restore",
                    &restore_layouts,
                    Some("fs_restore"),
                )
            },
        );
        Self {
            clear,
            restore,
            restore_layout,
        }
    }

    /// Clear the pass's viewport (set by the caller) to depth 1.
    pub(crate) fn clear<'p>(&'p self, pass: &mut wgpu::RenderPass<'p>) {
        pass.set_pipeline(&self.clear);
        pass.draw(0..3, 0..1);
    }
}

/// The sun cascades' cache on the GPU (see the module docs).
pub(crate) struct SunCacheGpu {
    pub(crate) state: SunCache,
    pub(crate) fill: FillGpu,
    /// The static atlas (the frame atlas's size; created when a cascade
    /// first has dynamic casters) and the restore bind group over it.
    pub(crate) statics: Option<(u32, wgpu::TextureView, wgpu::BindGroup)>,
    /// This frame's plan per cascade and its caster packets, sorted for
    /// submission (`frame::submit`): the dynamic casters, and the static
    /// ones of a cascade that redraws them this frame (empty otherwise).
    pub(crate) plans: [TilePlan; MAX_CASCADES],
    pub(crate) static_lists: [Vec<Draw>; MAX_CASCADES],
    pub(crate) dynamic_lists: [Vec<Draw>; MAX_CASCADES],
    /// The entity key of each draw of the frame's visible entities (the
    /// loc draws' identity; scratch kept for its capacity).
    pub(crate) draw_keys: Vec<Option<EntityKey>>,
    /// This frame's class of each draw and of each shadow-only draw (the
    /// point shadows read them too).
    pub(crate) draw_classes: Vec<CasterClass>,
    pub(crate) only_classes: Vec<CasterClass>,
    /// The dynamic locs seen: their key and the frame it last changed.
    pub(crate) dynamic_locs: crate::fast_hash::FastMap<LocSlot, (EntityKey, u64)>,
    /// The scene those keys belong to (`ModernRenderer::scene_token`).
    pub(crate) scene: Option<(usize, usize)>,
    /// The off-screen casters prepared so far, by scene slot.
    pub(crate) prepared: crate::fast_hash::FastMap<LocSlot, PreparedCaster>,
    /// Tests: draw every caster into every tile each frame, static and
    /// dynamic in one pass over a cleared atlas (the frame before the
    /// caches).
    #[cfg(test)]
    pub(crate) test_uncached: bool,
}

impl SunCacheGpu {
    pub(crate) fn new(device: &wgpu::Device, module: &wgpu::ShaderModule) -> Self {
        Self {
            state: SunCache::default(),
            fill: FillGpu::new(device, module),
            statics: None,
            plans: [TilePlan::default(); MAX_CASCADES],
            static_lists: Default::default(),
            dynamic_lists: Default::default(),
            draw_keys: Vec::new(),
            draw_classes: Vec::new(),
            only_classes: Vec::new(),
            dynamic_locs: Default::default(),
            scene: None,
            prepared: Default::default(),
            #[cfg(test)]
            test_uncached: false,
        }
    }
}

/// The static atlas of `size` texels and its restore bind group.
fn static_atlas(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    size: u32,
) -> (u32, wgpu::TextureView, wgpu::BindGroup) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("modern shadow static atlas"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: crate::shadows::SHADOW_FORMAT,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("modern shadow restore"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&view),
        }],
    });
    (size, view, bind)
}

/// The model identity `prepare_entity` keys a loc mesh on (its
/// fingerprint: the model's address and counts).
fn fingerprint(entity: &crate::models::draw_list::EntityDraw<'_>) -> (usize, i32, i32) {
    (
        entity.model as *const _ as usize,
        entity.model.unique_count,
        entity.model.draw_face_count,
    )
}

/// Whether the dynamic loc of `key` counts as a dynamic caster this frame
/// (its model changed within `DYNAMIC_SETTLE` frames), recording its key.
fn animating(
    dynamic_locs: &mut crate::fast_hash::FastMap<LocSlot, (EntityKey, u64)>,
    key: EntityKey,
    now: u64,
) -> bool {
    if key.revision.is_none() {
        return false;
    }
    let seen = dynamic_locs
        .entry(crate::frame::resources::loc_slot(&key))
        .or_insert((key, 0));
    if seen.0 != key {
        *seen = (key, now);
    }
    seen.1 != 0 && now - seen.1 < crate::shadows::cache::DYNAMIC_SETTLE
}

impl ModernRenderer {
    /// The off-screen casters of this frame (`shadows::casters`, with the
    /// roof-hidden entities): each one's draws prepared into the
    /// shadow-only draws, except the static ones prepared before with the
    /// same model, which are returned with their hash (`shadows::cache`: a
    /// frame whose cascades keep their maps does not need their draws;
    /// [`Self::plan_shadow_casters`] prepares them when it does).
    pub(crate) fn gather_shadow_casters<'a>(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'a>,
        hidden: Option<&crate::shadows::interior::RoofHidden>,
        origin: [f32; 3],
    ) -> Vec<OffScreenCaster<'a>> {
        self.scene_resources.interior.hidden_draws = 0;
        self.history.shadow.sun.only_classes.clear();
        let Some(frame) = self.frame_resources.shadow_frame.as_ref() else {
            return Vec::new();
        };
        if self.history.shadow.sun.scene != self.scene_resources.scene_token {
            self.history.shadow.sun.scene = self.scene_resources.scene_token;
            self.history.shadow.sun.dynamic_locs.clear();
            self.history.shadow.sun.prepared.clear();
        }
        let settings = self.shadow_settings();
        let settings = (settings.scenery, settings.characters);
        let now = self.history.frame;
        let casters =
            crate::shadows::casters::off_screen_parallel(snapshot, frame, hidden, &self.jobs);
        // Their loc meshes not cached yet, built on the threads
        // (`frame::prebuild`).
        self.prebuild_locs(snapshot, casters.iter().map(|(entity, _, _)| entity));
        let mut deferred = Vec::new();
        for (entity, mask, roof) in casters {
            // The entity's hash: its model, scene-local matrix and kind.
            let hash = entity.key.map(|key| {
                let mut h = Mix::tagged(5);
                std::hash::Hash::hash(&key, &mut h);
                let (address, unique, faces) = fingerprint(&entity);
                h.word(address as u64);
                h.word(u64::from(unique as u32) << 32 | u64::from(faces as u32));
                for v in entity.matrix {
                    h.word(u64::from(v.to_bits()));
                }
                h.word(
                    u64::from(entity.depth_write)
                        | u64::from(settings.0) << 1
                        | u64::from(settings.1) << 2,
                );
                std::hash::Hasher::finish(&h)
            });
            let dynamic = match entity.key {
                Some(key) => animating(&mut self.history.shadow.sun.dynamic_locs, key, now),
                None => true,
            };
            let known = entity.key.and_then(|key| {
                self.history
                    .shadow
                    .sun
                    .prepared
                    .get(&crate::frame::resources::loc_slot(&key))
                    .filter(|p| {
                        p.key == key
                            && p.fingerprint == fingerprint(&entity)
                            && p.settings == settings
                    })
                    .copied()
            });
            let hash = hash.filter(|_| !dynamic);
            if let (Some(known), Some(_)) = (known, hash) {
                if !known.scrolls {
                    if roof {
                        self.scene_resources.interior.hidden_draws += known.draws;
                    }
                    if known.draws > 0 {
                        deferred.push(OffScreenCaster {
                            entity,
                            mask,
                            hash,
                            draws: known.draws,
                        });
                    }
                    continue;
                }
            }
            let caster = OffScreenCaster {
                entity,
                mask,
                hash,
                draws: 0,
            };
            let (draws, scrolls) =
                self.prepare_off_screen(device, queue, snapshot, &caster, origin);
            let entity = caster.entity;
            if roof {
                self.scene_resources.interior.hidden_draws += draws;
            }
            if let Some(key) = entity.key {
                self.history.shadow.sun.prepared.insert(
                    crate::frame::resources::loc_slot(&key),
                    PreparedCaster {
                        key,
                        fingerprint: fingerprint(&entity),
                        settings,
                        draws,
                        scrolls,
                    },
                );
            }
        }
        deferred
    }

    /// Prepare one off-screen caster's draws into the shadow-only draws with
    /// its cascades, classed static by its hash (its first static draw
    /// carries it; a draw whose material scrolls is dynamic) or, without a
    /// hash, dynamic. Returns its casting draws and whether one scrolls (a
    /// caster the next frames must prepare).
    fn prepare_off_screen(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        caster: &OffScreenCaster<'_>,
        origin: [f32; 3],
    ) -> (usize, bool) {
        let (mask, hash) = (caster.mask, caster.hash);
        let start = self.frame_resources.draws.len();
        let bounds = self.prepare_entity(device, queue, snapshot, &caster.entity, origin);
        let before = self.frame_resources.shadow_only.len();
        let instances = &self.frame_resources.instances;
        let scrolls = self.frame_resources.draws[start..].iter().any(|d| {
            d.casts && {
                let p1 = instances[d.instance as usize].p1;
                p1[0] != 0.0 || p1[1] != 0.0
            }
        });
        self.frame_resources.shadow_only.extend(
            self.frame_resources
                .draws
                .drain(start..)
                .filter(|d| d.casts)
                .map(|d| (d, mask)),
        );
        let draws = self.frame_resources.shadow_only.len() - before;
        self.frame_resources
            .shadow_only_bounds
            .extend(std::iter::repeat_n(bounds, draws));
        let classes = &mut self.history.shadow.sun.only_classes;
        let mut first = true;
        for (d, _) in &self.frame_resources.shadow_only[before..] {
            let p1 = self.frame_resources.instances[d.instance as usize].p1;
            classes.push(match hash {
                Some(_) if p1[0] != 0.0 || p1[1] != 0.0 => CasterClass::Dynamic,
                Some(h) if first => {
                    first = false;
                    CasterClass::Static(h)
                }
                Some(_) => CasterClass::StaticPart,
                None => CasterClass::Dynamic,
            });
        }
        (draws, scrolls)
    }

    /// Each caster's class (`shadows::cache`) and, per sun cascade, the
    /// static casters' signature, the cascade's plan and its caster packets
    /// (after the frame's draws and cascade masks; `visible` holds the
    /// visible entities' draw ranges `(first, end, id)`; the off-screen
    /// casters were classed by [`Self::gather_shadow_casters`], and its
    /// `deferred` ones are prepared here when a cascade or the point shadows
    /// redraw their static casters). Returns the caster draws that meet a
    /// cascade (over every cascade) and those this frame encodes.
    pub(crate) fn plan_shadow_casters(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
        visible: &[(usize, usize, usize)],
        deferred: Vec<OffScreenCaster<'_>>,
    ) -> (usize, usize) {
        let terrain_casts = self.terrain_casts();
        let sun = &mut self.history.shadow.sun;
        for k in 0..MAX_CASCADES {
            sun.static_lists[k].clear();
            sun.dynamic_lists[k].clear();
        }
        sun.plans = [TilePlan::default(); MAX_CASCADES];
        sun.draw_classes.clear();
        let Some(frame) = self.frame_resources.shadow_frame.as_ref() else {
            return (0, 0);
        };
        let n = frame.profile.cascades;
        let all = (1_u8 << n) - 1;
        let now = self.history.frame;
        // The visible loc draws' keys.
        sun.draw_keys.clear();
        sun.draw_keys.resize(self.frame_resources.draws.len(), None);
        for &(start, end, id) in visible {
            let key = snapshot.entity_key(id);
            let end = end.min(sun.draw_keys.len());
            for k in &mut sun.draw_keys[start.min(end)..end] {
                *k = key;
            }
        }
        // The floors' identities: their level, upload and tile selection.
        let floor_ids: Vec<Option<Mix>> = self
            .scene_resources
            .floors
            .iter()
            .enumerate()
            .map(|(level, f)| {
                f.as_ref().map(|f| {
                    let mut h = Mix::tagged(2);
                    h.word(level as u64);
                    match &f.token {
                        crate::frame::resources::FloorToken::Calls(c) => {
                            h.word(std::sync::Arc::as_ptr(c) as usize as u64);
                        }
                        crate::frame::resources::FloorToken::Stream(p, n) => {
                            h.word(*p as u64);
                            h.word(*n as u64);
                        }
                    }
                    if let Some(s) = &f.selection {
                        selection_word(&mut h, s);
                    }
                    h
                })
            })
            .collect();
        let textures = &self.device_resources.textures;
        let instances = &self.frame_resources.instances;
        let dynamic_locs = &mut sun.dynamic_locs;
        let draw_keys = &sun.draw_keys;
        let mut classify = |i: usize, d: &Draw| -> CasterClass {
            if !d.casts || textures.get(d.material).is_none() {
                return CasterClass::None;
            }
            let instance = &instances[d.instance as usize];
            // A scrolling material's alpha test moves with the clock.
            if instance.p1[0] != 0.0 || instance.p1[1] != 0.0 {
                return CasterClass::Dynamic;
            }
            let identity = match d.geometry {
                // The far scene's merged batches never cast (P6); treat
                // any that did as redrawn every frame.
                Geometry::Arena { .. } | Geometry::Far { .. } => return CasterClass::Dynamic,
                Geometry::Loc { page, base_vertex } => {
                    // A loc mesh is rewritten in place on a new model, so
                    // its identity is its entity's key (a loc drawn by no
                    // visible entity is redrawn every frame).
                    let Some(key) = draw_keys.get(i).copied().flatten() else {
                        return CasterClass::Dynamic;
                    };
                    if animating(dynamic_locs, key, now) {
                        return CasterClass::Dynamic;
                    }
                    let mut h = Mix::tagged(1);
                    std::hash::Hash::hash(&key, &mut h);
                    h.word(u64::from(page) << 32 | u64::from(base_vertex as u32));
                    h
                }
                Geometry::Floor { level, batch } => {
                    let Some(Some(mut h)) = floor_ids.get(level).copied() else {
                        return CasterClass::None;
                    };
                    h.word(batch as u64);
                    h
                }
            };
            CasterClass::Static(draw_hash(
                identity,
                d.material,
                (d.first_index, d.count),
                &instance.model,
                instance.p0,
                origin,
            ))
        };
        let mut signatures = [Signature::default(); MAX_CASCADES];
        let mut met = 0;
        let mut add = |class: CasterClass, mask: u8| {
            let mask = mask & all;
            met += mask.count_ones() as usize;
            if let CasterClass::Static(h) = class {
                for (_, s) in signatures[..n]
                    .iter_mut()
                    .enumerate()
                    .filter(|(k, _)| mask & (1 << k) != 0)
                {
                    s.add(h);
                }
            }
        };
        for (i, d) in self.frame_resources.draws.iter().enumerate() {
            let class = classify(i, d);
            sun.draw_classes.push(class);
            add(class, self.frame_resources.cascade_masks[i]);
        }
        for (j, (_, mask)) in self.frame_resources.shadow_only.iter().enumerate() {
            add(
                sun.only_classes
                    .get(j)
                    .copied()
                    .unwrap_or(CasterClass::Dynamic),
                *mask,
            );
        }
        for d in &deferred {
            let mask = d.mask & all;
            met += mask.count_ones() as usize * d.draws;
            for (_, s) in signatures[..n]
                .iter_mut()
                .enumerate()
                .filter(|(k, _)| mask & (1 << k) != 0)
            {
                s.add(d.hash.unwrap_or_default());
            }
        }
        // The terrain and the roof-hidden tiles: drawn into every cascade.
        let t = &self.scene_resources.terrain;
        if terrain_casts {
            if let Some(scene) = t.scene.as_ref().filter(|_| !t.draws.is_empty()) {
                let mut h = Mix::tagged(3);
                h.word(scene.key.base[0] as u32 as u64);
                h.word(scene.key.base[1] as u32 as u64);
                for f in &scene.key.floors {
                    std::hash::Hash::hash(f, &mut h);
                }
                for &(level, count) in &t.draws {
                    h.word(level as u64);
                    h.word(u64::from(count));
                    if let Some(Some(gpu)) = scene.levels.get(level) {
                        if let Some(s) = &gpu.selection {
                            selection_word(&mut h, s);
                        }
                    }
                }
                let h = std::hash::Hasher::finish(&h);
                for s in &mut signatures[..n] {
                    s.add(h);
                }
            }
        }
        let i = &self.scene_resources.interior;
        if !i.terrain.is_empty() {
            let mut h = Mix::tagged(4);
            for &(level, count) in &i.terrain {
                h.word(level as u64);
                h.word(u64::from(count));
                if let Some(Some(hidden)) = i.levels.get(level) {
                    selection_word(&mut h, &hidden.selection);
                }
            }
            let h = std::hash::Hasher::finish(&h);
            for s in &mut signatures[..n] {
                s.add(h);
            }
        }
        // Which cascades have dynamic casters.
        let mut dynamic = 0_u8;
        let dynamic_masks = self
            .frame_resources
            .draws
            .iter()
            .zip(&sun.draw_classes)
            .zip(&self.frame_resources.cascade_masks)
            .map(|((_, c), m)| (*c, *m))
            .chain(
                self.frame_resources
                    .shadow_only
                    .iter()
                    .zip(&sun.only_classes)
                    .map(|((_, m), c)| (*c, *m)),
            );
        for (class, mask) in dynamic_masks {
            if class == CasterClass::Dynamic {
                dynamic |= mask & all;
            }
        }
        // The static atlas (the frame atlas's size) when a cascade has
        // dynamic casters: a new one holds nothing yet.
        let size = self.history.shadow.atlas.0;
        if dynamic != 0 && sun.statics.as_ref().is_none_or(|s| s.0 != size) {
            sun.statics = Some(static_atlas(device, &sun.fill.restore_layout, size));
            sun.state.forget_cache();
        }
        for (k, signature) in signatures.iter().enumerate().take(n) {
            sun.plans[k] = sun.state.plan(k, *signature, dynamic & (1 << k) != 0);
        }
        #[cfg(test)]
        if sun.test_uncached {
            sun.state.reset();
            for plan in &mut sun.plans[..n] {
                *plan = TilePlan {
                    statics_to_atlas: true,
                    dynamic: true,
                    ..TilePlan::default()
                };
            }
        }
        // The cascades that redraw their static casters.
        let redraw: u8 = (0..n)
            .filter(|&k| sun.plans[k].statics_to_cache || sun.plans[k].statics_to_atlas)
            .fold(0, |m, k| m | 1 << k);
        // The deferred casters' draws, those in a cascade that redraws its
        // static casters, or within reach of a point-light shadow (their
        // hashes are in the signatures already).
        let wanted: Vec<bool> = deferred
            .iter()
            .map(|d| {
                let m = &d.entity.matrix;
                let at = [m[12] - origin[0], m[13] - origin[1], m[14] - origin[2]];
                d.mask & redraw != 0
                    || self.history.shadow.point.reaches(
                        &self.scene_resources.lights.frame,
                        origin,
                        at,
                    )
            })
            .collect();
        for (d, wanted) in deferred.iter().zip(wanted) {
            if wanted {
                self.prepare_off_screen(device, queue, snapshot, d, origin);
            }
        }
        // The packets: every cascade's dynamic casters, the static ones of
        // the cascades that redraw them; sorted for submission.
        let sun = &mut self.history.shadow.sun;
        let casters = self
            .frame_resources
            .draws
            .iter()
            .zip(
                sun.draw_classes
                    .iter()
                    .zip(&self.frame_resources.cascade_masks),
            )
            .map(|(d, (c, m))| (d, *c, *m))
            .chain(
                self.frame_resources
                    .shadow_only
                    .iter()
                    .zip(&sun.only_classes)
                    .map(|((d, m), c)| (d, *c, *m)),
            );
        for (d, class, mask) in casters {
            let (lists, mask) = match class {
                CasterClass::None => continue,
                CasterClass::Dynamic => (&mut sun.dynamic_lists, mask & all),
                CasterClass::Static(_) | CasterClass::StaticPart => {
                    (&mut sun.static_lists, mask & redraw)
                }
            };
            for list in lists[..n]
                .iter_mut()
                .enumerate()
                .filter(|(k, _)| mask & (1 << k) != 0)
                .map(|(_, l)| l)
            {
                list.push(*d);
            }
        }
        #[cfg(test)]
        let sorted = !self.preparation.test_unsorted_packets;
        #[cfg(not(test))]
        let sorted = true;
        let mut encoded = 0;
        for k in 0..n {
            if sorted {
                sun.static_lists[k].sort_by_key(crate::frame::submit::sort_key);
                sun.dynamic_lists[k].sort_by_key(crate::frame::submit::sort_key);
            }
            encoded += sun.static_lists[k].len();
            if sun.plans[k].dynamic {
                encoded += sun.dynamic_lists[k].len();
            }
        }
        (met, encoded)
    }
}

/// Cascade `k`'s tile (`res` texels) as the pass's viewport and scissor.
fn set_tile(pass: &mut wgpu::RenderPass<'_>, k: usize, res: u32) {
    let (u, v) = crate::shadows::tile_origin(k);
    let (tx, ty) = ((u * 2.0) as u32 * res, (v * 2.0) as u32 * res);
    pass.set_viewport(tx as f32, ty as f32, res as f32, res as f32, 0.0, 1.0);
    pass.set_scissor_rect(tx, ty, res, res);
}

impl<'a> EncodeInputs<'a> {
    /// The sun shadow passes (pass type 0): the static casters of the
    /// cascades whose static atlas tile is stale, then each cascade's frame
    /// atlas tile brought up to date (`shadows::cache`: kept, restored from
    /// the static atlas, redrawn, with the dynamic casters on top).
    pub(crate) fn encode_sun_shadows(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(frame) = &self.shadow_frame else {
            return;
        };
        let sun = &self.shadow.sun;
        let n = frame.profile.cascades;
        let res = frame.profile.resolution;
        let plans = &sun.plans[..n];
        if let Some((_, view, _)) = sun.statics.as_ref() {
            let redraw: Vec<usize> = (0..n).filter(|&k| plans[k].statics_to_cache).collect();
            if !redraw.is_empty() {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(self.begin_pass(crate::frame::passes::Pass::SunShadowsStatic)),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: if redraw.len() == n {
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
                for &k in &redraw {
                    set_tile(&mut pass, k, res);
                    if redraw.len() != n {
                        sun.fill.clear(&mut pass);
                    }
                    self.encode_static_casters(&mut pass, k);
                }
            }
        }
        if !plans.iter().any(TilePlan::touches_atlas) {
            return;
        }
        // Every tile redrawn from scratch: one clear of the atlas.
        let whole = plans.iter().all(|p| p.statics_to_atlas);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some(self.begin_pass(crate::frame::passes::Pass::SunShadows)),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.shadow.atlas.1,
                depth_ops: Some(wgpu::Operations {
                    load: if whole {
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
        for (k, plan) in plans.iter().enumerate() {
            if !plan.touches_atlas() {
                continue;
            }
            set_tile(&mut pass, k, res);
            if plan.restore {
                if let Some((_, _, bind)) = sun.statics.as_ref() {
                    pass.set_pipeline(&sun.fill.restore);
                    pass.set_bind_group(0, bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            } else if plan.statics_to_atlas && !whole {
                sun.fill.clear(&mut pass);
            }
            if plan.statics_to_atlas {
                self.encode_static_casters(&mut pass, k);
            }
            if plan.dynamic {
                self.begin_casters(&mut pass, k);
                self.submit_all(&mut pass, &sun.dynamic_lists[k]);
            }
        }
    }

    /// Cascade `k`'s static casters (the terrain, locs and floors, the
    /// roof-hidden tiles) into `pass`, its tile set.
    fn encode_static_casters<'p>(&self, pass: &mut wgpu::RenderPass<'p>, k: usize) {
        self.begin_casters(pass, k);
        if self.terrain_casts() {
            self.encode_terrain(pass, TerrainPass::Shadow);
            pass.set_pipeline(&self.shadow.pipeline);
        }
        self.submit_all(pass, &self.shadow.sun.static_lists[k]);
        self.encode_interior(pass);
    }

    /// The caster pipeline and cascade `k`'s groups 0 and 2.
    fn begin_casters<'p>(&self, pass: &mut wgpu::RenderPass<'p>, k: usize) {
        pass.set_pipeline(&self.shadow.pipeline);
        pass.set_bind_group(0, self.frame_bind, &[]);
        pass.set_bind_group(
            2,
            &self.shadow.caster_bind,
            &[(k as u64 * crate::shadows::CASTER_SLOT) as u32],
        );
    }
}
