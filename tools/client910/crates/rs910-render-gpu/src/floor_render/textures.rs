//! Floor material texture caching, gamma conversion and mip uploads.

use std::collections::HashMap;

use crate::cache::Pack;

use crate::texture::{AlphaMode, Material, MaterialStore};

// `FloorUniforms` and `GAME_TO_WORLD` moved to rs910-scene (Phase 3.2).

/// One material's GPU texture + sampler (a cache entry).
pub struct FloorTexture {
    pub(super) view: wgpu::TextureView,
    /// Shared by every texture with the same sampler state
    /// ([`FloorTextureCache`]).
    pub(super) sampler: std::sync::Arc<wgpu::Sampler>,
}

/// Material id → texture cache.
#[derive(Default)]
pub struct FloorTextureCache {
    entries: rs910_core::soft_cache::SoftMap<i32, FloorTexture>,
    /// Samplers by `(repeat_s, repeat_t, mipmaps)` (programme Phase 6: one
    /// per sampler state instead of one per material texture).
    samplers: HashMap<(bool, bool, bool), std::sync::Arc<wgpu::Sampler>>,
}

impl FloorTextureCache {
    pub(super) fn get_or_load(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pack: &Pack,
        materials: &MaterialStore,
        material: i32,
    ) -> anyhow::Result<&FloorTexture> {
        if !self.entries.contains_key(&material) {
            let samplers = &mut self.samplers;
            let mut sampler = |repeat_s: bool, repeat_t: bool, mipmaps: bool| {
                samplers
                    .entry((repeat_s, repeat_t, mipmaps))
                    .or_insert_with(|| {
                        std::sync::Arc::new(make_sampler(device, repeat_s, repeat_t, mipmaps))
                    })
                    .clone()
            };
            let tex = if material == -1 {
                white_texture(device, queue, &mut sampler)
            } else {
                let m = materials
                    .get(material as u32)
                    .ok_or_else(|| anyhow::anyhow!("floor material {material} missing"))?;
                match material_texture(device, queue, pack, m, &mut sampler) {
                    Ok(t) => t,
                    Err(err) => {
                        log::warn!("[floor] material {material}: {err:#}; using white");
                        white_texture(device, queue, &mut sampler)
                    }
                }
            };
            self.entries.insert(material, tex);
        }
        Ok(self
            .entries
            .get(&material)
            .expect("the texture was just stored"))
    }
}

impl FloorTextureCache {
    /// Age the textures by one clean (see `rs910_core::soft_cache`).
    pub fn clean(&mut self, age: u32) {
        self.entries.clean(age);
    }

    /// Drop the textures no mesh has asked for a while; how many went.
    pub fn clear_soft(&mut self) -> usize {
        self.entries.clear_soft()
    }

    /// How many textures are cached.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no texture is cached.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl FloorTextureCache {
    /// A material's texture and sampler; `-1` or an unloadable material is
    /// the white texture.
    pub fn texture_or_white(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pack: &Pack,
        materials: &MaterialStore,
        material: i32,
    ) -> anyhow::Result<(&wgpu::TextureView, &wgpu::Sampler)> {
        let texture = self.get_or_load(device, queue, pack, materials, material)?;
        Ok((&texture.view, &*texture.sampler))
    }
}

/// The gamma look-up table (0.7 for material textures).
pub(super) fn gamma_lut(gamma: f64) -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let v = ((i as f64 / 255.0).powf(gamma) * 255.0) as i32;
        *slot = if v > 255 { 255 } else { v as u8 };
    }
    lut
}

/// Exact 2x2 integer box mip from ARGB ints.
pub(super) fn box_mip(src: &[u32], w: usize, h: usize) -> Vec<u32> {
    let (w2, h2) = (w >> 1, h >> 1);
    let mut out = Vec::with_capacity(w2 * h2);
    let mut row0 = 0;
    let mut row1 = w;
    for _ in 0..h2 {
        for _ in 0..w2 {
            let p0 = src[row0];
            let p1 = src[row0 + 1];
            let p2 = src[row1];
            let p3 = src[row1 + 1];
            row0 += 2;
            row1 += 2;
            let sum = |shift: u32| {
                ((p0 >> shift) & 0xFF)
                    + ((p1 >> shift) & 0xFF)
                    + ((p2 >> shift) & 0xFF)
                    + ((p3 >> shift) & 0xFF)
            };
            let a = sum(24);
            let r = sum(16);
            let g = sum(8);
            let b = sum(0);
            out.push(
                ((a & 0x3FC) << 22) | ((r & 0x3FC) << 14) | ((g & 0x3FC) << 6) | ((b >> 2) & 0xFF),
            );
        }
        row0 += w;
        row1 += w;
    }
    out
}

/// Upload an ARGB-int image with its integer box mip chain as `Rgba8Unorm`.
pub(super) fn upload_argb(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    label: &str,
    w: u32,
    h: u32,
    argb: Vec<u32>,
    mipmaps: bool,
) -> wgpu::TextureView {
    let levels = if mipmaps {
        32 - w.min(h).max(1).leading_zeros()
    } else {
        1
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut level_px = argb;
    let (mut lw, mut lh) = (w as usize, h as usize);
    for level in 0..levels {
        if level > 0 {
            level_px = box_mip(&level_px, lw, lh);
            lw >>= 1;
            lh >>= 1;
        }
        let mut bytes = Vec::with_capacity(lw * lh * 4);
        for p in &level_px {
            bytes.extend_from_slice(&[
                ((p >> 16) & 0xFF) as u8,
                ((p >> 8) & 0xFF) as u8,
                (p & 0xFF) as u8,
                ((p >> 24) & 0xFF) as u8,
            ]);
        }
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &bytes,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * lw as u32),
                rows_per_image: Some(lh as u32),
            },
            wgpu::Extent3d {
                width: lw as u32,
                height: lh as u32,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn make_sampler(
    device: &wgpu::Device,
    repeat_s: bool,
    repeat_t: bool,
    mipmaps: bool,
) -> wgpu::Sampler {
    let wrap = |r: bool| {
        if r {
            wgpu::AddressMode::Repeat
        } else {
            wgpu::AddressMode::ClampToEdge
        }
    };
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("floor material"),
        address_mode_u: wrap(repeat_s),
        address_mode_v: wrap(repeat_t),
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: if mipmaps {
            wgpu::MipmapFilterMode::Linear
        } else {
            wgpu::MipmapFilterMode::Nearest
        },
        ..Default::default()
    })
}

/// The 1x1 white texture.
pub(super) fn white_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    sampler: &mut dyn FnMut(bool, bool, bool) -> std::sync::Arc<wgpu::Sampler>,
) -> FloorTexture {
    let view = upload_argb(device, queue, "floor white", 1, 1, vec![0xFFFF_FFFF], false);
    FloorTexture {
        view,
        sampler: sampler(true, true, false),
    }
}

/// One material's texture (the non-bloom path).
pub(super) fn material_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pack: &Pack,
    m: &Material,
    sampler: &mut dyn FnMut(bool, bool, bool) -> std::sync::Arc<wgpu::Sampler>,
) -> anyhow::Result<FloorTexture> {
    let id = m
        .diffuse_texture
        .ok_or_else(|| anyhow::anyhow!("material {} has no diffuse texture", m.id))?;
    let image = crate::texture::load_png(pack, id, 0)?;
    let lut = gamma_lut(0.7);
    let opaque_path = m.alpha == AlphaMode::None && m.effect != 1 && m.effect != 7;
    let mut argb = Vec::with_capacity((image.w * image.h) as usize);
    for px in image.px.chunks_exact(4) {
        // Gamma on RGB, alpha kept.
        let r = u32::from(lut[px[0] as usize]);
        let g = u32::from(lut[px[1] as usize]);
        let b = u32::from(lut[px[2] as usize]);
        let mut a = u32::from(px[3]);
        if opaque_path {
            // Alpha 0 when RGB == 0, else 255.
            a = if (r | g | b) == 0 { 0 } else { 0xFF };
        }
        argb.push((a << 24) | (r << 16) | (g << 8) | b);
    }
    let edge = m
        .size
        .ok_or_else(|| anyhow::anyhow!("material {} has invalid size", m.id))?;
    anyhow::ensure!(
        argb.len() >= (edge * edge) as usize,
        "material texture shorter than its declared size"
    );
    argb.truncate((edge * edge) as usize);
    let pow2 = edge.is_power_of_two();
    let view = upload_argb(device, queue, "floor material", edge, edge, argb, pow2);
    Ok(FloorTexture {
        view,
        sampler: sampler(m.repeat_s == 1, m.repeat_t == 1, pow2),
    })
}
