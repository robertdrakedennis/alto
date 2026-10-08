//! The faithful scene passes of one frame: the underwater set, the opaque entities, every
//! level's floor with its static lights and hard shadows, the transparent
//! entities with their particles and billboards.

use super::*;

impl Renderer {
    /// The underwater models drawn in one list, in order.
    pub(super) fn underwater_list(
        &self,
        transparent: bool,
    ) -> Vec<&crate::floor_render::FloorMesh> {
        match &self.underwater_order {
            Some((opaque, blended)) => (if transparent { blended } else { opaque })
                .iter()
                .filter_map(|&i| self.underwater_models.get(i).map(|(mesh, _)| mesh))
                .collect(),
            None => self
                .underwater_models
                .iter()
                .filter(|(_, t)| *t == transparent)
                .map(|(mesh, _)| mesh)
                .collect(),
        }
    }

    /// The scene target this frame draws into (multisampling, bloom or a
    /// live post-process capture need one), recreated when the size or the
    /// sample count changed.
    pub(super) fn ensure_scene_target(
        &mut self,
        gpu: &crate::gpu_device::Device,
        post_bounds: Option<[u32; 4]>,
    ) {
        if (self.scene_samples > 1 || self.bloom_enabled || post_bounds.is_some())
            && !self
                .scene_target
                .as_ref()
                .is_some_and(|t| t.matches([self.size.0, self.size.1], self.scene_samples))
        {
            let format = self.scene_format(gpu);
            self.scene_target = Some(crate::scene_target::Target::with_pipelines(
                &gpu.device,
                [self.size.0, self.size.1],
                format,
                DEPTH_FORMAT,
                self.scene_samples,
                crate::scene_target::CopyPipelines::cached(
                    &self.pipelines,
                    &gpu.device,
                    format,
                    self.scene_samples,
                ),
            ));
        }
    }

    /// This frame's scene lists in draw chunks (see [`SceneLists`]), with
    /// each chunk's render bundle rebuilt only when its meshes changed.
    pub(super) fn scene_lists<'a>(
        &mut self,
        gpu: &crate::gpu_device::Device,
        floors: &'a [crate::floorpass::FloorLevelDraw<'a>],
        scene_opaque: &'a [&'a crate::floor_render::FloorMesh],
        scene_transparent: &'a [&'a crate::floor_render::FloorMesh],
        scene_format: wgpu::TextureFormat,
        bundle_models: bool,
    ) -> SceneLists<'a> {
        // Each particle owner's `drawParticles` follows its entity's models:
        // split both lists after those entities.
        // Each model's billboards follow its own batches.
        let chunks = |list: u8, len: usize| -> Vec<(usize, usize)> {
            let mut chunks = Vec::new();
            let mut start = 0;
            let mut splits = self.particles.splits(list, len);
            splits.extend(self.billboards.splits(list, len));
            splits.sort_unstable();
            splits.dedup();
            for end in splits {
                chunks.push((start, end));
                start = end;
            }
            if start < len || chunks.is_empty() {
                chunks.push((start, len));
            }
            chunks
        };
        let chunks = [
            chunks(0, scene_opaque.len()),
            chunks(1, scene_transparent.len()),
        ];
        if bundle_models {
            for ((cached, meshes), chunks) in [
                (&mut self.scene_opaque_bundle, scene_opaque),
                (&mut self.scene_transparent_bundle, scene_transparent),
            ]
            .into_iter()
            .zip(&chunks)
            {
                cached.truncate(chunks.len());
                for (k, &(start, end)) in chunks.iter().enumerate() {
                    let part = &meshes[start..end];
                    if cached.get(k).is_some_and(|bundle| bundle.matches(part)) {
                        continue;
                    }
                    let bundle = crate::floor_render::ModelBundle::new(
                        &gpu.device,
                        &self.floor,
                        part,
                        scene_format,
                        DEPTH_FORMAT,
                    );
                    if k < cached.len() {
                        cached[k] = bundle;
                    } else {
                        cached.push(bundle);
                    }
                }
            }
        }
        SceneLists {
            floors,
            scene_opaque,
            scene_transparent,
            chunks,
            bundle_models,
        }
    }

    /// The skybox pre-pass (the skybox replaces the fog-colour clear) when
    /// this frame draws a scene and has a sky; returns whether it drew.
    pub(super) fn encode_sky_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        env: &crate::env::EnvFrame,
    ) -> bool {
        let sky_drawn = self.sky_drawn();
        if sky_drawn {
            // The skybox replaces the fog-colour clear.
            self.env_passes.encode_sky(
                encoder,
                &self.floor,
                &crate::skybox_render::SkyTargets {
                    colour: self.scene_target.as_ref().map_or(view, |t| t.attachment()),
                    resolve: self.scene_target.as_ref().and_then(|t| t.resolve()),
                    depth: self
                        .scene_target
                        .as_ref()
                        .map_or(&self.depth_view, |t| &t.depth),
                    colour_load: if self.ui.is_some() && self.scene_target.is_none() {
                        wgpu::LoadOp::Load
                    } else {
                        wgpu::LoadOp::Clear(wgpu::Color {
                            r: f64::from(env.clear[0]),
                            g: f64::from(env.clear[1]),
                            b: f64::from(env.clear[2]),
                            a: 1.0,
                        })
                    },
                    rect: self
                        .ui
                        .as_ref()
                        .and_then(|ui| ui.scene.as_ref())
                        .map(|scene| (scene.rect, scene.clip)),
                },
            );
        }
        sky_drawn
    }

    /// Whether [`Renderer::encode_sky_pass`] draws the sky this frame.
    pub(super) fn sky_drawn(&self) -> bool {
        self.ui.as_ref().is_none_or(|ui| ui.scene.is_some())
            && !self.scene_blackout
            && self.env_passes.has_sky()
    }

    /// The scene pass: the underwater set, the opaque entities, every level's floor with its
    /// static lights and hard shadows, then the transparent entities, each
    /// chunk followed by its billboards and particles.
    pub(super) fn encode_scene_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        env: &crate::env::EnvFrame,
        sky_drawn: bool,
        lists: &SceneLists<'_>,
    ) {
        let SceneLists {
            floors,
            scene_opaque,
            scene_transparent,
            ref chunks,
            bundle_models,
        } = *lists;
        if self.ui.as_ref().is_none_or(|ui| ui.scene.is_some()) && !self.scene_blackout {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("multi-level pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: self.scene_target.as_ref().map_or(view, |t| t.attachment()),
                    resolve_target: self.scene_target.as_ref().and_then(|t| t.resolve()),
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // Without a sky the target clears to the fog colour.
                        load: if sky_drawn || (self.ui.is_some() && self.scene_target.is_none()) {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color {
                                r: f64::from(env.clear[0]),
                                g: f64::from(env.clear[1]),
                                b: f64::from(env.clear[2]),
                                a: 1.0,
                            })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: self
                        .scene_target
                        .as_ref()
                        .map_or(&self.depth_view, |t| &t.depth),
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: self
                    .frame_profile
                    .as_ref()
                    .and_then(|p| p.timestamp_writes()),
            });
            if let Some(scene) = self.ui.as_ref().and_then(|ui| ui.scene.as_ref()) {
                let [x, y, w, h] = scene.rect;
                pass.set_viewport(x as f32, y as f32, w as f32, h as f32, 0., 1.);
                let [l, t, r, b] = scene.clip;
                pass.set_scissor_rect(l as u32, t as u32, (r - l) as u32, (b - t) as u32);
            }
            // Opaque entities, then every level's floor, then transparent
            // entities (ordered by draw::DrawPlan).
            if !floors.is_empty() || !scene_opaque.is_empty() || !scene_transparent.is_empty() {
                pass.set_pipeline(self.floor.scene_pipeline());
                pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                let cache_model_pipeline =
                    !crate::render_debug_flags::flags().profile_redundant_pipelines;
                let mut current_alpha_test = Some((false, true));
                // The underwater set draws first (opaque entities, the
                // floor, then the transparent entities).
                for mesh in self.underwater_list(false) {
                    mesh.draw_model_cached(&mut pass, &self.floor, &mut current_alpha_test, false);
                }
                self.env_passes.draw_underwater(&mut pass);
                pass.set_pipeline(self.floor.scene_pipeline());
                pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                current_alpha_test = Some((false, true));
                for mesh in self.underwater_list(true) {
                    mesh.draw_model_cached(&mut pass, &self.floor, &mut current_alpha_test, false);
                }
                pass.set_pipeline(self.floor.scene_pipeline());
                pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                current_alpha_test = Some((false, true));
                for (k, &(start, end)) in chunks[0].iter().enumerate() {
                    if bundle_models {
                        pass.execute_bundles(std::iter::once(&self.scene_opaque_bundle[k].bundle));
                    } else {
                        for mesh in &scene_opaque[start..end] {
                            mesh.draw_model_cached(
                                &mut pass,
                                &self.floor,
                                &mut current_alpha_test,
                                cache_model_pipeline,
                            );
                        }
                    }
                    // An entity's particles draw after its models.
                    self.billboards.draw_at(
                        &mut pass,
                        &self.floor.uniform_bind_group,
                        0,
                        end,
                        scene_opaque.len(),
                    );
                    self.particles.draw_at(
                        &mut pass,
                        &self.floor.uniform_bind_group,
                        0,
                        end,
                        scene_opaque.len(),
                    );
                    pass.set_pipeline(self.floor.scene_pipeline());
                    pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                    current_alpha_test = Some((false, true));
                }
                // Per level: batches, then the static lights, then the hard
                // shadows.
                for level in floors {
                    if level.floor.vertex_count == 0 {
                        continue;
                    }
                    pass.set_pipeline(self.floor.scene_pipeline());
                    pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                    level.floor.draw(&mut pass);
                    if !level.lights.is_empty() {
                        pass.set_pipeline(&self.passes.light_pipeline);
                        pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                        for light in level.lights {
                            light.draw(&mut pass);
                        }
                    }
                    if let Some(shadows) = level.shadows {
                        pass.set_pipeline(&self.passes.shadow_pipeline);
                        pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                        shadows.draw(&mut pass);
                    }
                }
                pass.set_pipeline(self.floor.scene_pipeline());
                pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                // Floor/light/shadow passes changed state; the ordinary
                // floor pipeline was just rebound above.
                current_alpha_test = Some((false, true));
                for (k, &(start, end)) in chunks[1].iter().enumerate() {
                    if bundle_models {
                        pass.execute_bundles(std::iter::once(
                            &self.scene_transparent_bundle[k].bundle,
                        ));
                    } else {
                        for mesh in &scene_transparent[start..end] {
                            mesh.draw_model_cached(
                                &mut pass,
                                &self.floor,
                                &mut current_alpha_test,
                                cache_model_pipeline,
                            );
                        }
                    }
                    self.billboards.draw_at(
                        &mut pass,
                        &self.floor.uniform_bind_group,
                        1,
                        end,
                        scene_transparent.len(),
                    );
                    self.particles.draw_at(
                        &mut pass,
                        &self.floor.uniform_bind_group,
                        1,
                        end,
                        scene_transparent.len(),
                    );
                    pass.set_pipeline(self.floor.scene_pipeline());
                    pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
                    current_alpha_test = Some((false, true));
                }
            } else if self.particles.has_draws() {
                self.particles
                    .draw(&mut pass, &self.floor.uniform_bind_group);
            }
        }
    }

    /// The scene target into the frame: through the post-process chain when a capture is live, else as drawn.
    pub(super) fn encode_post_pass(
        &mut self,
        gpu: &crate::gpu_device::Device,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        scene_format: wgpu::TextureFormat,
        post_bounds: Option<[u32; 4]>,
    ) {
        if self.ui.as_ref().is_none_or(|ui| ui.scene.is_some()) && !self.scene_blackout {
            if let Some(target) = &self.scene_target {
                let clip = self.ui.as_ref().and_then(|ui| ui.scene.as_ref()).map_or(
                    [0, 0, self.size.0, self.size.1],
                    |s| {
                        let [l, t, r, b] = s.clip;
                        [l as u32, t as u32, (r - l) as u32, (b - t) as u32]
                    },
                );
                // A live chain resolves the capture into the layer bounds;
                // otherwise the scene is shown as drawn.
                let processed = post_bounds.is_some_and(|bounds| {
                    self.post.encode(
                        &gpu.device,
                        gpu,
                        encoder,
                        &crate::postprocess::Capture {
                            scene: &target.view,
                            format: scene_format,
                            size: [self.size.0, self.size.1],
                            scene_clip: clip,
                        },
                        &crate::postprocess::Screen {
                            view,
                            format: gpu.config.format,
                            bounds,
                        },
                    )
                });
                if processed {
                } else if scene_format != gpu.config.format {
                    // An HDR scene outside any capture is shown unprocessed.
                    self.post.present(
                        &gpu.device,
                        gpu,
                        encoder,
                        &target.view,
                        [self.size.0, self.size.1],
                        &crate::postprocess::Screen {
                            view,
                            format: gpu.config.format,
                            bounds: clip,
                        },
                    );
                } else {
                    target.present(encoder, view, clip);
                }
            }
        }
    }
}

/// One frame's scene lists in draw order (the `DrawPlan`
/// the shell resolved to meshes) and how they draw.
pub(super) struct SceneLists<'a> {
    pub(super) floors: &'a [crate::floorpass::FloorLevelDraw<'a>],
    pub(super) scene_opaque: &'a [&'a crate::floor_render::FloorMesh],
    pub(super) scene_transparent: &'a [&'a crate::floor_render::FloorMesh],
    /// Each list's `(start, end)` chunks: split after every particle or
    /// billboard owner, which draw after their entity's models.
    pub(super) chunks: [Vec<(usize, usize)>; 2],
    /// Draw each chunk as its cached render bundle (else mesh by mesh,
    /// `CLIENT910_PROFILE_DIRECT_DRAWS`).
    pub(super) bundle_models: bool,
}
