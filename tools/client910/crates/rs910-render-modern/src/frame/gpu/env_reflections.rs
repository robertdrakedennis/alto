//! The global environment cube of the modern client's environment
//! mapping on the GPU ([`crate::lighting::env_reflections`]): the forward pass's group 3
//! bindings 10-12 (the cube faded from, the cube faded to, their block),
//! the camera square's cube material loaded on first use and cross-faded
//! over 5000 ms when it changes.
use std::collections::HashMap;

use crate::frame::*;

/// The `GlobalEnv` block of `lighting::env_reflections`'s WGSL.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct GlobalEnvUniforms {
    /// x: the fade (the second cube's share); y: the record's environment
    /// mapping parameter ([`crate::lighting::env_reflections::PARAMS_W`]); z: 1 = a cube is bound; w: -.
    pub(crate) params: [f32; 4],
}

/// The group 3 entries (bindings 10-12).
pub(crate) fn layout_entries() -> [wgpu::BindGroupLayoutEntry; 3] {
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
    [
        cube(10),
        cube(11),
        wgpu::BindGroupLayoutEntry {
            binding: 12,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        },
    ]
}

/// A cube texture of six `size`² RGBA8 faces (+X, -X, +Y, -Y, +Z, -Z).
pub(crate) fn cube(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    size: u32,
    faces: &[&[u8]],
) -> wgpu::TextureView {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("modern global environment cube"),
        size: wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: 6,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (face, px) in faces.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: face as u32,
                },
                aspect: wgpu::TextureAspect::All,
            },
            px,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 4),
                rows_per_image: Some(size),
            },
            wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    })
}

/// See the module docs.
pub(crate) struct GlobalEnvGpu {
    pub(crate) uniforms: wgpu::Buffer,
    /// The 1-texel cube bound without a cube.
    pub(crate) empty: wgpu::TextureView,
    /// Loaded cubes by material id (`None`: not loadable).
    pub(crate) cubes: HashMap<u16, Option<wgpu::TextureView>>,
    /// The fade: from, to, its start (the logic clock).
    pub(crate) from: Option<u16>,
    pub(crate) to: Option<u16>,
    pub(crate) start_ms: i64,
    /// The ids the bind group holds.
    pub(crate) bound: (Option<u16>, Option<u16>),
}

impl GlobalEnvGpu {
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
    ) -> Self {
        let black: &[u8] = &[0, 0, 0, 255];
        let empty = cube(device, queue, 1, &[black; 6]);
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("modern global environment"),
            size: std::mem::size_of::<GlobalEnvUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Self {
            uniforms,
            empty,
            cubes: HashMap::new(),
            from: None,
            to: None,
            start_ms: 0,
            bound: (None, None),
        }
    }

    pub(crate) fn view(&self, id: Option<u16>) -> &wgpu::TextureView {
        id.and_then(|id| self.cubes.get(&id)?.as_ref())
            .unwrap_or(&self.empty)
    }

    /// The group 3 entries (bindings 10-12).
    pub(crate) fn entries(&self) -> [wgpu::BindGroupEntry<'_>; 3] {
        let (a, b) = self.bound;
        [
            wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(self.view(a)),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: wgpu::BindingResource::TextureView(self.view(b)),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: self.uniforms.as_entire_binding(),
            },
        ]
    }

    /// Cube material `id` loaded (a material with the cube flag, its
    /// texture's six faces; its scale float is in no 910 material: 1).
    pub(crate) fn load(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        pack: &crate::cache::Pack,
        materials: &crate::texture::MaterialStore,
        id: u16,
    ) {
        self.cubes.entry(id).or_insert_with(|| {
            let loaded = (|| {
                let m = materials
                    .get(u32::from(id))
                    .ok_or_else(|| anyhow::anyhow!("no material"))?;
                anyhow::ensure!(m.environment_cube, "not a cube material");
                let texture = m
                    .diffuse_texture
                    .ok_or_else(|| anyhow::anyhow!("no texture"))?;
                let faces = crate::texture::load_cube(pack, texture, 0)?;
                let size = faces[0].w;
                anyhow::ensure!(
                    faces.iter().all(|f| f.w == size
                        && f.h == size
                        && f.px.len() >= (size * size * 4) as usize),
                    "unequal faces"
                );
                let px: Vec<&[u8]> = faces
                    .iter()
                    .map(|f| &f.px[..(size * size * 4) as usize])
                    .collect();
                Ok(cube(device, queue, size, &px))
            })();
            match loaded {
                Ok(view) => {
                    log::info!("[modern] global environment cube: material {id}");
                    Some(view)
                }
                Err(error) => {
                    log::warn!("[modern] global environment cube material {id}: {error:#}");
                    None
                }
            }
        });
    }

    /// This frame's cube: `target` (the camera square's id), faded from the
    /// previous target over [`crate::lighting::env_reflections::CUBE_FADE_MS`] at
    /// `now_ms`. Returns whether the bind group must be rebuilt.
    pub(crate) fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        target: Option<u16>,
        now_ms: i64,
    ) -> bool {
        if let (Some(id), Some(pack), Some(materials)) = (target, snapshot.pack, snapshot.materials)
        {
            self.load(device, queue, pack, materials, id);
        }
        let target = target.filter(|id| self.cubes.get(id).is_some_and(Option::is_some));
        if target != self.to {
            // On an environment change the old target becomes the cube faded
            // from, the new one the cube faded to.
            self.from = self.to.filter(|_| target.is_some());
            self.to = target;
            self.start_ms = now_ms;
        }
        let mut t = ((now_ms - self.start_ms) as f32
            / crate::lighting::env_reflections::CUBE_FADE_MS as f32)
            .clamp(0.0, 1.0);
        if t >= 1.0 || self.from.is_none() {
            self.from = None;
            t = 1.0;
        }
        let bound = (self.from.or(self.to), self.to);
        let uniforms = GlobalEnvUniforms {
            params: [
                t,
                crate::lighting::env_reflections::PARAMS_W,
                if self.to.is_some() { 1.0 } else { 0.0 },
                0.0,
            ],
        };
        queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));
        let rebind = bound != self.bound;
        self.bound = bound;
        rebind
    }
}

impl ModernRenderer {
    /// The camera square's global environment cube.
    pub(crate) fn prepare_global_env(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        snapshot: &SceneSnapshot<'_>,
        now_ms: i64,
    ) {
        let target = self.environment.global_cube(snapshot);
        if self
            .scene_resources
            .lights
            .global_env
            .prepare(device, queue, snapshot, target, now_ms)
        {
            self.scene_resources.lights.rebind(device);
        }
    }
}
