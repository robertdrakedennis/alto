//! Upload paths: floor levels, lights, hard shadows and scene models
//! their per-frame tile selections, materials, uniforms and model updates, the underwater
//! scene, the skybox, particles and model billboards. The mesh owners
//! (`scene_meshes::SceneMeshes`, `player_renderer::PlayersRenderer`) call
//! these at the client's points.

use super::*;

impl Renderer {
    /// Upload this frame's skybox (`sky`, resolved by the shell's
    /// `sky_frame::SkyCache`; `None` = clear to the fog colour) for the
    /// pre-pass. Returns the boxes whose model this
    /// toolkit could not create (`skybox_render::EnvPasses::prepare`).
    pub fn prepare_skybox(
        &mut self,
        gpu: &crate::gpu_device::Device,
        assets: &crate::skybox_render::SkyAssets<'_>,
        sky: Option<crate::sky_frame::SkyFrame<'_>>,
        camera: &crate::camera::SceneCamera,
        env: &crate::env::EnvFrame,
    ) -> Vec<crate::skybox::SkyboxKey> {
        let viewport = self.scene_size();
        self.env_passes.prepare(
            crate::skybox_render::SkyUploads {
                device: &gpu.device,
                queue: gpu,
                floor: &self.floor,
                textures: &mut self.floor_textures,
                assets,
                camera,
                env,
                viewport,
            },
            sky,
        )
    }

    /// The uploaded underwater floor.
    pub fn underwater_floor_mut(&mut self) -> &mut Option<crate::floor_render::FloorMesh> {
        &mut self.env_passes.underwater
    }

    /// Tile selection of the underwater floor (the frame's level-0 mask).
    pub fn select_underwater_tiles(
        &mut self,
        gpu: &crate::gpu_device::Device,
        geometry: &crate::floor::FloorGeometry,
        selection: &crate::draw::FloorSelection,
    ) {
        if let Some(mesh) = self.env_passes.underwater.as_mut() {
            mesh.select_tiles(
                gpu,
                geometry,
                &selection.tiles(geometry.tiles_x, geometry.tiles_z),
            );
        }
    }

    /// Material walk of the underwater pass: opaque entities, the floor
    /// (under underwater fog), then transparent entities.
    pub fn prepare_underwater_materials(
        &mut self,
        gpu: &crate::gpu_device::Device,
        state: &mut crate::material::MaterialState,
        millis: i32,
    ) {
        for (mesh, _) in self.underwater_models.iter_mut().filter(|(_, t)| !t) {
            mesh.prepare_materials(gpu, state, millis, &[[0.; 4]; 8], 0);
        }
        if let Some(mesh) = self.env_passes.underwater.as_mut() {
            mesh.prepare_materials(gpu, state, millis, &[[0.; 4]; 8], 0);
        }
        for (mesh, _) in self.underwater_models.iter_mut().filter(|(_, t)| *t) {
            mesh.prepare_materials(gpu, state, millis, &[[0.; 4]; 8], 0);
        }
    }

    /// The culled, depth-sorted underwater lists, as indices into the
    /// uploaded models.
    pub fn set_underwater_order(&mut self, order: Option<(Vec<usize>, Vec<usize>)>) {
        self.underwater_order = order;
    }

    /// Upload the underwater scene's loc models with their per-object water
    /// fog.
    pub fn set_underwater_models(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        models: &[crate::rebuild::UnderwaterModel],
        base: (i32, i32),
    ) -> anyhow::Result<()> {
        self.underwater_models.clear();
        for m in models {
            let origin = [
                (base.0 * 512 + m.position[0]) as f32,
                m.position[1] as f32,
                (base.1 * 512 + m.position[2]) as f32,
            ];
            let mut mesh =
                self.build_model_mesh(gpu, pack, materials, &m.model, origin, "underwater loc")?;
            mesh.set_height_fog(gpu, m.fog_plane, m.fog_colour);
            self.underwater_models.push((mesh, m.transparent));
        }
        Ok(())
    }

    /// One toolkit timestamp shared by every material in the frame.
    pub fn material_time(&mut self) -> i32 {
        self.floor.begin_material_frame()
    }

    pub fn update_model_mesh(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floor_render::FloorMesh,
        materials: &crate::texture::MaterialStore,
        model: &crate::gpumodel::GpuModel,
    ) -> anyhow::Result<bool> {
        mesh.update_model(gpu, materials, model)
    }

    /// [`crate::floor_render::FloorMesh::reuse_for_model`]: last frame's
    /// transient mesh rewritten for this frame's model at `origin_fine`, or
    /// false when it does not fit.
    pub fn reuse_model_mesh(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floor_render::FloorMesh,
        materials: &crate::texture::MaterialStore,
        model: &crate::gpumodel::GpuModel,
        origin_fine: [f32; 3],
    ) -> anyhow::Result<bool> {
        mesh.reuse_for_model(gpu, materials, model, origin_fine)
    }

    /// Place this frame's particle owner segments in the scene lists:
    /// `ends[list][i]` is the scene-mesh count up to and including plan
    /// entity `i` of that list.
    pub fn resolve_particle_segments(&mut self, ends: [&[usize]; 2]) {
        self.particles.resolve_segments(ends);
    }

    /// Place and upload this frame's model billboards for the ordered scene lists: each mesh's quads follow
    /// it in its list. `fog` is `fogStart/fogEnd` when fog is on.
    pub fn prepare_billboards(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        camera: &crate::camera::SceneCamera,
        fog: Option<(f32, f32)>,
        lists: [&[&crate::floor_render::FloorMesh]; 2],
    ) -> anyhow::Result<()> {
        let mut local = camera.clone();
        local.target = [0; 3];
        let view = crate::billboard_render::View::new(
            local.view_entries(),
            &local.projection(),
            fog,
            self.bloom_enabled,
        );
        let target = camera.target.map(f64::from);
        let mut frame = crate::billboard_render::Frame::default();
        for (list, meshes) in lists.iter().enumerate() {
            for (index, mesh) in meshes.iter().enumerate() {
                if let Some(b) = &mesh.billboards {
                    frame.add(list as u8, index + 1, b, &b.world(target), &view);
                }
            }
        }
        if crate::render_debug_flags::flags().billboard_trace {
            let owners: Vec<String> = lists
                .iter()
                .flat_map(|l| l.iter())
                .filter_map(|m| m.billboards.as_ref())
                .map(|b| {
                    let p = b.matrix.point(0.0, 0.0, 0.0);
                    format!(
                        "{}x@({:.0},{:.0},{:.0})",
                        b.instances.len(),
                        b.origin[0] + f64::from(p[0]),
                        f64::from(p[1]) + b.origin[1],
                        b.origin[2] + f64::from(p[2])
                    )
                })
                .collect();
            let vp = crate::camera::multiply(&view.view, &local.projection());
            let quads: Vec<String> = frame
                .draws
                .iter()
                .map(|d| {
                    let v = &frame.vertices[d.first_vertex as usize..d.first_vertex as usize + 4];
                    let c: [f32; 3] =
                        std::array::from_fn(|i| v.iter().map(|v| v.pos[i]).sum::<f32>() / 4.0);
                    let clip: [f32; 4] = std::array::from_fn(|j| {
                        c[0] * vp[j] + c[1] * vp[4 + j] + c[2] * vp[8 + j] + vp[12 + j]
                    });
                    let w = (v[2].pos[0] - v[0].pos[0]).hypot(v[2].pos[2] - v[0].pos[2]);
                    format!(
                        "m{} rgba{:?} ndc({:.2},{:.2}) span{:.0} fog{:.2}",
                        d.material,
                        v[0].colour,
                        clip[0] / clip[3],
                        clip[1] / clip[3],
                        w,
                        v[0].fog
                    )
                })
                .collect();
            log::info!(
                "[billboards] owners {} quads {} segments {} target {:?} {:?} {:?}",
                owners.len(),
                frame.draws.len(),
                frame.segments.len(),
                camera.target,
                owners.iter().take(12).collect::<Vec<_>>(),
                quads.iter().take(12).collect::<Vec<_>>()
            );
        }
        self.billboards.prepare(
            &gpu.device,
            gpu,
            &mut self.floor_textures,
            pack,
            materials,
            &frame,
        )
    }

    /// Upload this frame's particle batches.
    pub fn prepare_particles(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        frame: &crate::particle_render::Frame,
    ) -> anyhow::Result<()> {
        self.particles.prepare(
            &gpu.device,
            gpu,
            &mut self.floor_textures,
            pack,
            materials,
            frame,
        )
    }

    pub fn set_actor_frame(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floor_render::FloorMesh,
        camera: &crate::camera::SceneCamera,
        base: (i32, i32),
        env: &crate::env::EnvFrame,
        matrix: &crate::actor_matrix::Matrix,
    ) {
        // The actor matrix is scene-local (`actor_render::uniforms` moves
        // the camera by the base instead).
        if let Some(b) = mesh.billboards.as_mut() {
            b.matrix = *matrix;
            b.origin = [f64::from(base.0 * 512), 0.0, f64::from(base.1 * 512)];
        }
        let mut uniforms = crate::actor_render::uniforms(camera, base, env, matrix);
        uniforms.eye_time[3] = (self.floor.frame_millis() % 128000) as f32 / 1000.;
        mesh.set_model_uniforms(&gpu.device, gpu, &self.floor, &uniforms);
    }

    pub fn update_shadow_mesh(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floorpass::ShadowMesh,
        geometry: &crate::floor::FloorGeometry,
        dirty: &std::collections::HashSet<(usize, usize)>,
    ) {
        if let Some(mask) = crate::floor::shadow_mask_view(geometry) {
            mesh.update_mask(&gpu.queue, &mask, dirty);
        }
    }

    pub fn set_material_environment(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        id: i32,
        base: [f32; 3],
    ) -> anyhow::Result<()> {
        if self
            .floor
            .set_environment(&gpu.device, &gpu.queue, pack, materials, id, base)?
        {
            self.scene_opaque_bundle.clear();
            self.scene_transparent_bundle.clear();
        }
        Ok(())
    }

    pub fn prepare_materials(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floor_render::FloorMesh,
        state: &mut crate::material::MaterialState,
        millis: i32,
        lights: &[[f32; 4]; 8],
        count: usize,
    ) {
        mesh.prepare_materials(gpu, state, millis, lights, count);
    }

    pub fn select_floor_tiles(
        &self,
        gpu: &crate::gpu_device::Device,
        mesh: &mut crate::floor_render::FloorMesh,
        geometry: &crate::floor::FloorGeometry,
        lights: &mut [crate::floorpass::LightMesh],
        shadows: Option<&mut crate::floorpass::ShadowMesh>,
        selection: &crate::draw::FloorSelection,
    ) {
        mesh.select_tiles(
            gpu,
            geometry,
            &selection.tiles(geometry.tiles_x, geometry.tiles_z),
        );
        for light in lights {
            light.select_tiles(geometry, selection);
        }
        if let Some(shadow) = shadows {
            shadow.select_tiles(gpu, geometry, selection);
        }
    }

    /// Upload one finished floor level (`FloorGeometry`) for the faithful
    /// floor pipeline; material textures are loaded from `pack` on first use.
    pub fn build_floor_mesh(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        geometry: &crate::floor::FloorGeometry,
        origin_fine: [f32; 3],
        label: &str,
    ) -> anyhow::Result<crate::floor_render::FloorMesh> {
        crate::floor_render::FloorMesh::build(
            crate::floor_render::MeshUpload {
                device: &gpu.device,
                queue: &gpu.queue,
                pipeline: &self.floor,
                textures: &mut self.floor_textures,
                pack,
                materials,
            },
            geometry,
            origin_fine,
            label,
        )
    }

    /// Upload one level's baked static lights.
    pub fn build_light_meshes(
        &self,
        gpu: &crate::gpu_device::Device,
        lights: &[crate::floorlight::BakedLight],
        origin_fine: [f32; 3],
        label: &str,
    ) -> Vec<crate::floorpass::LightMesh> {
        lights
            .iter()
            .filter_map(|l| {
                crate::floorpass::LightMesh::build(&gpu.device, &self.passes, l, origin_fine, label)
            })
            .collect()
    }

    pub fn update_light_intensities(
        &self,
        gpu: &crate::gpu_device::Device,
        meshes: &[Vec<crate::floorpass::LightMesh>],
        values: &[f32],
    ) {
        for mesh in meshes.iter().flatten() {
            if let Some(id) = mesh.source_light {
                mesh.set_intensity(gpu, values[id]);
            }
        }
    }

    /// Upload one level's hard-shadow blocks.
    pub fn build_shadow_mesh(
        &self,
        gpu: &crate::gpu_device::Device,
        geometry: &crate::floor::FloorGeometry,
        mask: &crate::floorpass::ShadowMaskView<'_>,
        origin_fine: [f32; 3],
        label: &str,
    ) -> crate::floorpass::ShadowMesh {
        crate::floorpass::ShadowMesh::build(
            &gpu.device,
            &gpu.queue,
            &self.passes,
            geometry,
            mask,
            origin_fine,
            label,
        )
    }

    /// Upload one lit scene-graph model for the faithful model path (same
    /// pipeline as the floors). `origin_fine` is the entity's world
    /// translation in fine units.
    pub fn build_model_mesh(
        &mut self,
        gpu: &crate::gpu_device::Device,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        model: &crate::gpumodel::GpuModel,
        origin_fine: [f32; 3],
        label: &str,
    ) -> anyhow::Result<crate::floor_render::FloorMesh> {
        crate::floor_render::FloorMesh::from_model(
            crate::floor_render::MeshUpload {
                device: &gpu.device,
                queue: &gpu.queue,
                pipeline: &self.floor,
                textures: &mut self.floor_textures,
                pack,
                materials,
            },
            model,
            origin_fine,
            label,
        )
    }
}
