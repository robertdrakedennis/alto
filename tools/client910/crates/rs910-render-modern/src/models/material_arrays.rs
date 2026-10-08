//! The RT5 materials' maps as texture arrays, so a pass binds them once.
//!
//! A draw that changes its material changes a bind group: wgpu validates and
//! tracks it and Metal re-emits its texture state, about 60% of the CPU time
//! of a frame's draws (the headless bench's Lumbridge view sets 5,200 of
//! them, most between draws that differ only in the texture). The RT5
//! diffuse maps are 64, 128 or 256 texels square (each with its full mip
//! chain) and their aux maps the same: the 128 texel maps are layers of one
//! 2D array (and the aux maps of another), and so are the 64 texel maps,
//! tiled twice over each way. The layer, and whether it is tiled, ride the
//! draw's instance record (the high bits of its flags, [`slot_bits`]), so a
//! model draw binds nothing of its material: the arrays and their sampler
//! are group 4 of the model pipelines, set once per pass.
//!
//! The shader samples the layer with the same explicit mip levels and the
//! same addressing (repeat on both axes, the sampler every array material
//! takes) as the material's own texture, so the frame is the one the
//! material's own texture draws. A 64 texel map tiled 2x2 is the 64 texel
//! map seen at half its coordinates: each mip level is the tiling of that
//! level of the original, the texel positions are the same floats, the wrap
//! is the same. Materials that do not fit (clamped or single-axis repeat,
//! 256 texels, RT7 atlases, past the layers) and the billboards and
//! particles keep their own textures and their own bind group
//! ([`crate::models::materials::MaterialEntry::bind_group`], group 1).
//! Layer 0 is never a material (slot 0 means "own textures").
//!
//! One array pair and one sampler, rather than a pair per size and a
//! sampler per addressing mode, because the shader then selects nothing at
//! run time: choosing between textures or samplers by a value that differs
//! from draw to draw costs the GPU more than the frame's 5,000 bind group
//! changes cost the CPU (about 0.4 ms of the Lumbridge frame's 8).

/// One mip level: width, height, bytes (`models::materials::Level`).
type Level = (u32, u32, Vec<u8>);

/// Texels per side of the array layers.
const SIZE: u32 = 128;
/// Mip levels of a layer (the full chain).
const LEVELS: u32 = 8;
/// Layers, layer 0 (unused) included. Also the most the slot bits name.
const LAYERS: u32 = 512;

/// The layer count a device must offer (`max_texture_array_layers`).
pub const LAYERS_NEEDED: u32 = LAYERS;

/// Bit positions of the slot fields in a draw's flags (`p0.w` of the
/// instance record; the flags proper use bits 0 to 7, and 23 bits stay
/// exact in an `f32`).
const SLOT_SHIFT: u32 = 10;
const TILED_SHIFT: u32 = 19;

/// The flags bits of a material in layer `layer` (`tiled`: a 64 texel map,
/// tiled twice over each way in its layer).
#[must_use]
pub(crate) fn slot_bits(layer: u32, tiled: bool) -> u32 {
    debug_assert!(layer > 0 && layer < LAYERS);
    (layer << SLOT_SHIFT) | (u32::from(tiled) << TILED_SHIFT)
}

/// See the module docs.
pub struct MaterialArrays {
    pub(crate) layout: wgpu::BindGroupLayout,
    pub(crate) bind: wgpu::BindGroup,
    diffuse: wgpu::Texture,
    aux: wgpu::Texture,
    /// Layers of the arrays: [`LAYERS`], or what the device offers.
    layers: u32,
    next: u32,
    /// The layer of an all-white, no-aux material (untextured draws and the
    /// materials whose map did not load); 0: none.
    white: u32,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    layers: u32,
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: layers,
        },
        mip_level_count: LEVELS,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Write `levels` (level 0 first, `texel_bytes` a texel) into layer `layer`
/// of `texture`.
fn write_layer(
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    texture: &wgpu::Texture,
    layer: u32,
    texel_bytes: u32,
    levels: &[Level],
) {
    for (level, (w, h, data)) in levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d {
                    x: 0,
                    y: 0,
                    z: layer,
                },
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * texel_bytes),
                rows_per_image: Some(*h),
            },
            wgpu::Extent3d {
                width: *w,
                height: *h,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// The 64 texel mip chain `levels` (7 levels) tiled twice over each way: the
/// 128 texel chain whose level k is the tiling of level k (level 7, one
/// texel, is the original's last level).
fn tiled(levels: &[Level], texel_bytes: usize) -> Vec<Level> {
    let mut out: Vec<Level> = levels
        .iter()
        .map(|(w, h, data)| {
            let (w, h) = (*w as usize, *h as usize);
            let row = w * texel_bytes;
            let mut tiled = Vec::with_capacity(data.len() * 4);
            for _ in 0..2 {
                for y in 0..h {
                    let line = &data[y * row..(y + 1) * row];
                    tiled.extend_from_slice(line);
                    tiled.extend_from_slice(line);
                }
            }
            (2 * w as u32, 2 * h as u32, tiled)
        })
        .collect();
    let (_, _, last) = levels.last().expect("a level");
    out.push((1, 1, last[..texel_bytes].to_vec()));
    out
}

impl MaterialArrays {
    /// The arrays (capped at the device's `max_texture_array_layers`) and
    /// the bind group over the repeat-both `sampler`.
    pub(crate) fn new(
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        sampler: &wgpu::Sampler,
    ) -> Self {
        // One layer is no capacity at all (layer 0 is never a material).
        let layers = LAYERS.min(device.limits().max_texture_array_layers).max(1);
        let diffuse = texture(
            device,
            "modern material diffuse layers",
            layers,
            wgpu::TextureFormat::Rgba8UnormSrgb,
        );
        let aux = texture(
            device,
            "modern material aux layers",
            layers,
            wgpu::TextureFormat::R8Unorm,
        );
        let entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern material arrays"),
            entries: &[
                entry(0),
                entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let (diffuse_view, aux_view) = (view(&diffuse), view(&aux));
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern material arrays"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&diffuse_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&aux_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });
        let mut arrays = Self {
            layout,
            bind,
            diffuse,
            aux,
            layers,
            next: 1,
            white: 0,
        };
        // The white layer (an all-white texel samples the same at every
        // level, so it stands for the 1x1 white texture).
        if layers > 1 {
            let levels: Vec<Level> = (0..LEVELS)
                .map(|l| {
                    let side = (SIZE >> l).max(1);
                    (side, side, vec![255; (side * side * 4) as usize])
                })
                .collect();
            write_layer(queue, &arrays.diffuse, 1, 4, &levels);
            arrays.next = 2;
            arrays.white = 1;
        }
        arrays
    }

    /// The layer a material takes: `(layer, tiled)` for its `diffuse` mip
    /// chain and `aux` map, written into the arrays; `None` when it fits
    /// none (the material draws with its own textures).
    pub(crate) fn place(
        &mut self,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        diffuse: &[Level],
        aux: Option<&[Level]>,
    ) -> Option<(u32, bool)> {
        let (w, h, _) = diffuse.first()?;
        let (tiles, levels) = match (*w, *h, diffuse.len()) {
            (128, 128, 8) => (false, 8),
            (64, 64, 7) => (true, 7),
            _ => return None,
        };
        if let Some(aux) = aux {
            let fits =
                aux.len() == levels && aux.first().is_some_and(|(aw, ah, _)| aw == w && ah == h);
            if !fits {
                return None;
            }
        }
        if self.next >= self.layers {
            return None;
        }
        let layer = self.next;
        self.next += 1;
        if tiles {
            write_layer(queue, &self.diffuse, layer, 4, &tiled(diffuse, 4));
            if let Some(aux) = aux {
                write_layer(queue, &self.aux, layer, 1, &tiled(aux, 1));
            }
        } else {
            write_layer(queue, &self.diffuse, layer, 4, diffuse);
            if let Some(aux) = aux {
                write_layer(queue, &self.aux, layer, 1, aux);
            }
        }
        Some((layer, tiles))
    }

    /// The white layer, when the device has one.
    pub(crate) fn white(&self) -> Option<(u32, bool)> {
        (self.white > 0).then_some((self.white, false))
    }

    /// Forget every material's layer (their texels stay until rewritten).
    pub(crate) fn reset(&mut self) {
        self.next = if self.white > 0 { 2 } else { 1 };
    }
}

#[cfg(test)]
mod tests {
    use super::{tiled, Level};

    /// The tiled chain has the 128 texel chain's shape and each level is the
    /// original's level tiled (the last one, 1x1, the original's last).
    #[test]
    fn a_tiled_chain_is_the_originals_levels_tiled() {
        let levels: Vec<Level> = (0..7)
            .map(|k| {
                let side = 64_u32 >> k;
                let bytes = (0..side * side * 4)
                    .map(|i| (i * 7 + k * 13) as u8)
                    .collect();
                (side, side, bytes)
            })
            .collect();
        let t = tiled(&levels, 4);
        assert_eq!(t.len(), 8);
        for (k, (w, h, data)) in t.iter().enumerate().take(7) {
            let (ow, _, orig) = &levels[k];
            assert_eq!((*w, *h), (2 * ow, 2 * ow), "level {k} size");
            let o = *ow as usize;
            for y in 0..2 * o {
                for x in 0..2 * o {
                    let at = (y * 2 * o + x) * 4;
                    let src = ((y % o) * o + x % o) * 4;
                    assert_eq!(
                        data[at..at + 4],
                        orig[src..src + 4],
                        "level {k} texel ({x},{y})"
                    );
                }
            }
        }
        assert_eq!((t[7].0, t[7].1), (1, 1));
        assert_eq!(t[7].2, levels[6].2);
    }
}
