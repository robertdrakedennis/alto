//! The minimap base on the GPU: the base sprites drawn through the floor
//! pipeline and registered with the UI painter as external images.

use super::*;

/// The world a minimap base is drawn from: the per-level floor geometry and
/// meshes, the region base (in tiles) and the region height in tiles.
pub struct MinimapWorld<'a> {
    pub geometries: &'a [Option<crate::floor::FloorGeometry>],
    pub meshes: &'a mut [Option<crate::floor_render::FloorMesh>],
    pub base: (i32, i32),
    pub size_z: i32,
}

impl Renderer {
    /// Render the minimap base: one texture of
    /// `plan.size` pixels (4 per tile plus a 48 px margin), cleared black,
    /// each level's visible floor tiles drawn top-down through the
    /// floor pipeline (tile (x, z) covers pixels
    /// `48 + 4x .. +4` and rows `48 + 4(sizeZ - 1 - z) .. +4`), then the wall
    /// and loc marks. Every level's mesh is re-selected to the plan's tiles;
    /// the caller restores the frame selection afterwards. Returns the id
    /// registered with the UI painter as [`crate::ui_paint::Image::External`].
    pub fn render_minimap_base(
        &mut self,
        gpu: &mut crate::gpu_device::Device,
        plan: &crate::minimap::BasePlan,
        world: MinimapWorld<'_>,
        env: &crate::env::EnvFrame,
    ) -> anyhow::Result<u64> {
        let MinimapWorld {
            geometries,
            meshes,
            base,
            size_z,
        } = world;
        let size = plan.size;
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 1,
        };
        let target = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("minimap base"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: gpu.config.format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        if crate::toolkit_debug_flags::flags().minimap_dump.is_some() {
            let tiles: Vec<(usize, usize)> = plan
                .level_tiles
                .iter()
                .map(|(l, t)| (*l, t.len()))
                .collect();
            log::info!("[minimap] base {size}px first_level {} tiles/level {tiles:?} marks {} base {:?} size_z {size_z}", plan.first_level, plan.marks.len(), base);
        }
        let depth = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("minimap depth"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let view = target.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        // World tiles (x east, y up, z north) -> base pixels; depth from height so
        // an upper floor wins the depth test like a nearer top-down fragment.
        let s = size as f32;
        let k = 8.0 / s;
        let tx = 2.0 * (48.0 - 4.0 * base.0 as f32) / s - 1.0;
        let ty = 1.0 - 2.0 * (48.0 + 4.0 * size_z as f32 + 4.0 * base.1 as f32) / s;
        let view_proj = glam::Mat4::from_cols(
            glam::Vec4::new(k, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, -0.001, 0.0),
            glam::Vec4::new(0.0, k, 0.0, 0.0),
            glam::Vec4::new(tx, ty, 0.5, 1.0),
        );
        self.floor.upload_minimap_uniforms(gpu, view_proj, env);
        for (level, tiles) in &plan.level_tiles {
            if let (Some(mesh), Some(geometry)) = (
                meshes.get_mut(*level).and_then(Option::as_mut),
                geometries.get(*level).and_then(Option::as_ref),
            ) {
                mesh.select_tiles(gpu, geometry, tiles);
            }
        }
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("minimap base"),
            });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("minimap floors"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(self.floor.base_pipeline());
            pass.set_bind_group(0, &self.floor.uniform_bind_group, &[]);
            for (level, tiles) in &plan.level_tiles {
                if tiles.is_empty() {
                    continue;
                }
                if let Some(mesh) = meshes.get(*level).and_then(Option::as_ref) {
                    if mesh.vertex_count == 0 {
                        continue;
                    }
                    mesh.draw(&mut pass);
                }
            }
        }
        // The wall and loc marks as 2D quads over the floors.
        let mut painter = crate::ui_paint::Painter::new([size, size]);
        for mark in &plan.marks {
            match mark {
                crate::minimap::Mark::Fill { rect, colour } => painter.fill(*rect, *colour)?,
                crate::minimap::Mark::Line { from, to, colour } => {
                    painter.line(*from, *to, *colour, 1)
                }
                crate::minimap::Mark::Icon { sprite, rect } => painter.scaled(sprite, *rect, -1)?,
                // Batch mode tints the white sprite with the colour's own
                // alpha whatever the blend, as Painter::fill does.
                crate::minimap::Mark::Add { rect, colour } => painter.fill(*rect, *colour)?,
            }
        }
        let plan2d = painter.finish();
        if !plan2d.quads.is_empty() {
            let mut marks = crate::ui_paint_gpu::Renderer::with_pipeline(
                crate::text_render_gpu::Pipeline::cached(
                    &self.pipelines,
                    &gpu.device,
                    gpu.config.format,
                ),
            );
            marks.prepare(&gpu.device, gpu, plan2d)?;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("minimap marks"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                multiview_mask: None,
                timestamp_writes: None,
            });
            marks.draw(&mut pass);
            drop(pass);
            gpu.submit(std::iter::once(encoder.finish()));
        } else {
            gpu.submit(std::iter::once(encoder.finish()));
        }
        if let Some(path) = crate::toolkit_debug_flags::flags().minimap_dump.clone() {
            let mut encoder = gpu
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("minimap dump"),
                });
            if let Some(capture) = gpu.capture_frame(&mut encoder, &target) {
                gpu.submit(std::iter::once(encoder.finish()));
                let saved = gpu.pending_screenshot.take();
                gpu.pending_screenshot = Some(std::path::PathBuf::from(path));
                gpu.finish_screenshot(capture);
                gpu.pending_screenshot = saved;
                gpu.screenshot_written = false;
            }
        }
        let id = self.next_external_id;
        self.next_external_id += 1;
        let ui = self
            .ui
            .get_or_insert_with(|| RetainedUi::new(gpu, &self.pipelines, &self.minimap_textures));
        ui.paint.register_external(&gpu.device, id, view.clone());
        if let Some(ui) = self.ui_spare.as_mut() {
            ui.paint.register_external(&gpu.device, id, view);
        }
        self.minimap_textures.insert(id, target);
        Ok(id)
    }

    /// Drop a base sprite.
    pub fn release_minimap_base(&mut self, id: u64) {
        self.minimap_textures.remove(&id);
        for ui in [&mut self.ui, &mut self.ui_spare].into_iter().flatten() {
            ui.paint.unregister_external(id);
        }
    }

    /// The id the next external image (a minimap base sprite) is registered
    /// under.
    pub fn next_external_id(&mut self) -> &mut u64 {
        &mut self.next_external_id
    }
}
