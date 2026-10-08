//! The static point lights' GPU state (renderer plan M4, [`crate::lighting::point_lights`]):
//! the forward pass's group 3 (the frame's lights, the tile index grid and
//! its uniforms), rebuilt when the scene's light table changes and
//! rewritten every frame (positions follow the camera origin, intensities
//! flicker).

use wgpu::util::DeviceExt;

use crate::frame::*;
use crate::lighting::point_lights::{GpuLight, Grid, GridStats, GridUniforms, Light};

/// The point-light bind group layout (group 3 of the forward pass; lane
/// Q-M6 appends the light probes' bindings 3-7, `probes`; the per-square
/// ambient's binding 13 comes with them).
pub(crate) fn point_light_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let [p3, p4, p5, p6, p7, p13] = crate::frame::gpu::probes::layout_entries();
    // The point-light shadows (bindings 8-9).
    let [s8, s9] = crate::frame::gpu::point_shadows::layout_entries();
    // The global environment cube (bindings 10-12).
    let [g10, g11, g12] = crate::frame::gpu::env_reflections::layout_entries();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("modern point lights"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
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
            p3,
            p4,
            p5,
            p6,
            p7,
            s8,
            s9,
            g10,
            g11,
            g12,
            p13,
        ],
    })
}

/// The retained installation identity, light count and grid size of a table; the
/// empty table is `(0, 0, (0, 0, 0))`.
pub(crate) type GridKey = (usize, usize, (usize, usize, usize));

/// See the module docs.
pub(crate) struct LightGpu {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) uniforms: wgpu::Buffer,
    /// The light records and their capacity in lights.
    pub(crate) buffer: (wgpu::Buffer, usize),
    pub(crate) grid: wgpu::Buffer,
    /// The installed grid's key and size `(levels, nx, nz)`.
    pub(crate) grid_key: Option<GridKey>,
    /// Keep the installation alive while its address identifies the GPU grid.
    grid_identity: Option<std::sync::Arc<()>>,
    pub(crate) grid_size: (usize, usize, usize),
    pub(crate) grid_stats: GridStats,
    pub(crate) bind: wgpu::BindGroup,
    /// The same without point shadows (a block of none), for the captures
    /// (the probes and the per-square ambient), which draw without them.
    pub(crate) bind_capture: wgpu::BindGroup,
    /// The frame's CPU lights (scene-local; tests and diagnostics).
    pub(crate) frame: Vec<Light>,
    pub(crate) scratch: Vec<GpuLight>,
    /// The light probes' bindings (`probes`).
    pub(crate) probes: crate::frame::gpu::probes::ProbeBindings,
    /// The point-light shadow maps (`point_shadow`).
    pub(crate) shadow_maps: crate::frame::gpu::point_shadows::PointShadowMaps,
    /// The global environment cube (the environment mapping).
    pub(crate) global_env: crate::frame::gpu::env_reflections::GlobalEnvGpu,
}

/// The grid as a storage buffer: per tile, row-major `(level * nz + z) *
/// nx + x`, the four `u16` slots (one empty tile for an empty grid).
pub(crate) fn grid_buffer(device: &wgpu::Device, grid: &Grid) -> wgpu::Buffer {
    let zero = [[0_u16; 4]];
    let entries: &[[u16; 4]] = if grid.entries.is_empty() {
        &zero
    } else {
        &grid.entries
    };
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("modern point light grid"),
        contents: bytemuck::cast_slice(entries),
        usage: wgpu::BufferUsages::STORAGE,
    })
}

pub(crate) fn light_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("modern point lights"),
        size: (capacity.max(1) * std::mem::size_of::<GpuLight>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

pub(crate) fn bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    // The light records, the tile grid and the grid block (bindings 0-2).
    [buffer, grid, uniforms]: [&wgpu::Buffer; 3],
    probes: &crate::frame::gpu::probes::ProbeBindings,
    // The point-shadow block and atlas (bindings 8-9).
    [s8, s9]: [wgpu::BindGroupEntry<'_>; 2],
    global_env: &crate::frame::gpu::env_reflections::GlobalEnvGpu,
) -> wgpu::BindGroup {
    let [p3, p4, p5, p6, p7, p13] = probes.entries();
    let [g10, g11, g12] = global_env.entries();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("modern point lights"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: grid.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniforms.as_entire_binding(),
            },
            p3,
            p4,
            p5,
            p6,
            p7,
            s8,
            s9,
            g10,
            g11,
            g12,
            p13,
        ],
    })
}

impl LightGpu {
    /// No lights: an empty grid and a zero light count.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        layout: wgpu::BindGroupLayout,
        shaders: &crate::shaders::Library,
    ) -> Self {
        let uniforms = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("modern point grid"),
            contents: bytemuck::bytes_of(&GridUniforms::default()),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let buffer = light_buffer(device, 1);
        let grid = grid_buffer(device, &Grid::default());
        let probes = crate::frame::gpu::probes::ProbeBindings::new(device, queue, shaders);
        let shadow_maps = crate::frame::gpu::point_shadows::PointShadowMaps::new(device, queue);
        let global_env = crate::frame::gpu::env_reflections::GlobalEnvGpu::new(device, queue);
        let bind = bind_group(
            device,
            &layout,
            [&buffer, &grid, &uniforms],
            &probes,
            shadow_maps.entries(),
            &global_env,
        );
        let bind_capture = bind_group(
            device,
            &layout,
            [&buffer, &grid, &uniforms],
            &probes,
            shadow_maps.entries_off(),
            &global_env,
        );
        Self {
            layout,
            uniforms,
            buffer: (buffer, 1),
            grid,
            grid_key: None,
            grid_identity: None,
            grid_size: (0, 0, 0),
            grid_stats: GridStats::default(),
            bind,
            bind_capture,
            frame: Vec::new(),
            scratch: Vec::new(),
            probes,
            shadow_maps,
            global_env,
        }
    }

    /// The installed light table's key (the empty table's
    /// before the first frame).
    pub(crate) fn key(&self) -> GridKey {
        self.grid_key.unwrap_or((0, 0, (0, 0, 0)))
    }

    /// The bind groups again (the point-shadow maps were recreated).
    pub(crate) fn rebind(&mut self, device: &wgpu::Device) {
        self.rebuild_binds(device);
    }

    /// Both bind groups over the current buffers.
    fn rebuild_binds(&mut self, device: &wgpu::Device) {
        let buffers = [&self.buffer.0, &self.grid, &self.uniforms];
        self.bind = bind_group(
            device,
            &self.layout,
            buffers,
            &self.probes,
            self.shadow_maps.entries(),
            &self.global_env,
        );
        self.bind_capture = bind_group(
            device,
            &self.layout,
            buffers,
            &self.probes,
            self.shadow_maps.entries_off(),
            &self.global_env,
        );
    }

    /// This frame's lights: the grid `grid` builds (installed when `key`
    /// changes) and `lights` (scene-local), written camera-local about
    /// `origin`.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        key: GridKey,
        grid: impl FnOnce() -> Grid,
        lights: Vec<Light>,
        origin: [f32; 3],
    ) {
        if Some(key) != self.grid_key {
            let grid = grid();
            self.grid = grid_buffer(device, &grid);
            self.grid_key = Some(key);
            self.grid_size = (grid.levels, grid.nx, grid.nz);
            self.grid_stats = grid.stats();
            self.rebuild_binds(device);
            log::info!(
                "[modern] point light grid: {} lights, {} levels x {} x {} tiles, {} lit, {} at the cap of {}",
                lights.len(),
                grid.levels,
                grid.nx,
                grid.nz,
                self.grid_stats.lit_tiles,
                self.grid_stats.full_tiles,
                crate::lighting::point_lights::MAX_PER_TILE
            );
        }
        if lights.len() > self.buffer.1 {
            let capacity = lights.len().next_power_of_two();
            self.buffer = (light_buffer(device, capacity), capacity);
            self.rebuild_binds(device);
        }
        self.scratch.clear();
        let strength = crate::lighting::point_lights::STRENGTH;
        self.scratch
            .extend(lights.iter().map(|l| l.gpu(origin, strength)));
        if !self.scratch.is_empty() {
            queue.write_buffer(&self.buffer.0, 0, bytemuck::cast_slice(&self.scratch));
        }
        let (levels, nx, nz) = self.grid_size;
        let uniforms = GridUniforms {
            origin: [origin[0], origin[1], origin[2], 0.0],
            dims: [nx as u32, nz as u32, levels as u32, lights.len() as u32],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        self.frame = lights;
    }
}

impl ModernRenderer {
    /// This frame's point lights (M4, `lighting::point_lights`): the snapshot's light
    /// table (`live.model_lights`: the classic lights with this frame's flicker
    /// intensity and group colour) as the grid and the light records;
    /// none without a live scene.
    pub(crate) fn prepare_lights(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        origin: [f32; 3],
    ) {
        let table = snapshot.live_frame().map(|live| live.model_lights);
        match table {
            Some(table) => {
                let key = (
                    std::sync::Arc::as_ptr(table.grid_identity()) as usize,
                    table.lights.len(),
                    table.grid_size(),
                );
                // The modern colour term (the colour clamped, then
                // times the intensity).
                let lights = crate::models::shading::frame_lights(table);
                self.scene_resources.lights.prepare(
                    device,
                    queue,
                    key,
                    || Grid::from_model_lights(table),
                    lights,
                    origin,
                );
                self.scene_resources.lights.grid_identity = Some(table.grid_identity().clone());
            }
            None => {
                self.scene_resources.lights.grid_identity = None;
                #[cfg(test)]
                if let Some((lights, grid)) = self.preparation.test_lights.clone() {
                    let key = (1, lights.len(), (grid.levels, grid.nx, grid.nz));
                    self.scene_resources.lights.prepare(
                        device,
                        queue,
                        key,
                        move || grid,
                        lights,
                        origin,
                    );
                    return;
                }
                self.scene_resources.lights.prepare(
                    device,
                    queue,
                    (0, 0, (0, 0, 0)),
                    Grid::default,
                    Vec::new(),
                    origin,
                );
            }
        }
    }
}
