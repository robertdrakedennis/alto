//! Environment, noise and water texture uploads and frame bindings.
use rs910_gpu_device::desc;

use crate::cache::Pack;

use crate::texture::Material;

// `FloorUniforms` and `GAME_TO_WORLD` moved to rs910-scene (Phase 3.2).

use super::box_mip;

pub(super) fn empty_cube(device: &wgpu::Device) -> wgpu::TextureView {
    let texture = device.create_texture(&desc::texture_layers(
        "null environment sampler",
        [1, 1, 6],
        wgpu::TextureFormat::Rgba8Unorm,
        wgpu::TextureUsages::TEXTURE_BINDING,
    ));
    texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    })
}

/// The group-0 textures and samplers shared by every material frame group.
pub(super) struct FrameViews<'a> {
    pub(super) cube: &'a wgpu::TextureView,
    pub(super) sampler: &'a wgpu::Sampler,
    pub(super) noise: &'a wgpu::TextureView,
    pub(super) noise_sampler: &'a wgpu::Sampler,
    pub(super) water_normals: &'a wgpu::TextureView,
}

pub(super) fn material_frame_bindings(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    views: FrameViews<'_>,
) -> wgpu::BindGroup {
    let FrameViews {
        cube,
        sampler,
        noise,
        noise_sampler,
        water_normals,
    } = views;
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("material frame"),
        layout,
        entries: &[
            desc::bind(0, uniforms.as_entire_binding()),
            desc::bind(1, wgpu::BindingResource::TextureView(cube)),
            desc::bind(2, wgpu::BindingResource::Sampler(sampler)),
            desc::bind(3, wgpu::BindingResource::TextureView(noise)),
            desc::bind(4, wgpu::BindingResource::Sampler(noise_sampler)),
            desc::bind(5, wgpu::BindingResource::TextureView(water_normals)),
            // The noise sampler is the same repeat/trilinear state.
            desc::bind(6, wgpu::BindingResource::Sampler(noise_sampler)),
        ],
    })
}

/// Upload a `128x128x16` volume plus the 2x2x2 integer box mips the
/// billow upload uses (`channels` bytes per texel).
pub(super) fn upload_volume_mips(
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    mut bytes: Vec<u8>,
    channels: usize,
) {
    let (mut w, mut h, mut d) = (128usize, 128usize, 16usize);
    for level in 0..8 {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some((w * channels) as u32),
                rows_per_image: Some(h as u32),
            },
            wgpu::Extent3d {
                width: w as u32,
                height: h as u32,
                depth_or_array_layers: d as u32,
            },
        );
        if level == 7 {
            break;
        }
        let (nw, nh, nd) = ((w / 2).max(1), (h / 2).max(1), (d / 2).max(1));
        let mut next = Vec::with_capacity(nw * nh * nd * channels);
        for z in 0..nd {
            for y in 0..nh {
                for x in 0..nw {
                    for channel in 0..channels {
                        let mut sum = 0u32;
                        for dz in 0..2 {
                            for dy in 0..2 {
                                for dx in 0..2 {
                                    sum += u32::from(
                                        bytes[(((z * 2 + dz).min(d - 1) * h
                                            + (y * 2 + dy).min(h - 1))
                                            * w
                                            + (x * 2 + dx).min(w - 1))
                                            * channels
                                            + channel],
                                    );
                                }
                            }
                        }
                        next.push(((sum + 4) / 8) as u8);
                    }
                }
            }
        }
        bytes = next;
        w = nw;
        h = nh;
        d = nd;
    }
}

/// Environment cube upload: gamma 1, six faces in +X,-X,+Y,-Y,+Z,-Z order; optional integer box mip chain.
pub(super) fn upload_environment_cube(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pack: &Pack,
    material: &Material,
) -> anyhow::Result<wgpu::TextureView> {
    let faces = crate::texture::load_cube(
        pack,
        material
            .diffuse_texture
            .ok_or_else(|| anyhow::anyhow!("cube has no texture"))?,
        0,
    )?;
    let size = material
        .size
        .ok_or_else(|| anyhow::anyhow!("invalid cube size"))?;
    let all_pixels: Vec<u32> = faces
        .iter()
        .flat_map(|f| {
            f.px.chunks_exact(4)
                .map(|p| u32::from_le_bytes(p.try_into().unwrap()))
        })
        .collect();
    anyhow::ensure!(
        all_pixels.len() >= (size * size * 6) as usize,
        "cube texture shorter than declared size"
    );
    let levels = if material.mip_mode != 0 {
        32 - size.leading_zeros()
    } else {
        1
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        mip_level_count: levels,
        ..desc::texture_layers(
            "environment cube",
            [size, size, 6],
            wgpu::TextureFormat::Rgba8Unorm,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        )
    });
    for face in 0..6 {
        let count = (size * size) as usize;
        let mut pixels = all_pixels[face * count..(face + 1) * count].to_vec();
        let mut width = size;
        for mip in 0..levels {
            desc::write_level(
                queue,
                &texture,
                mip,
                face as u32,
                [width, width],
                bytemuck::cast_slice(&pixels),
            );
            if mip + 1 < levels {
                pixels = box_mip(&pixels, width as usize, width as usize);
                width >>= 1;
            }
        }
    }
    Ok(texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    }))
}
