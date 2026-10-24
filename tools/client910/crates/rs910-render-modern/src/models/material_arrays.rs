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
use rs910_gpu_device::desc;

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
        mip_level_count: LEVELS,
        ..desc::texture_layers(
            label,
            [SIZE, SIZE, layers],
            format,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        )
    })
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
        let entry = |binding| {
            desc::float_texture(
                binding,
                wgpu::ShaderStages::FRAGMENT,
                true,
                wgpu::TextureViewDimension::D2Array,
            )
        };
        let layout = desc::layout(
            device,
            "modern material arrays",
            &[
                entry(0),
                entry(1),
                desc::sampler(
                    2,
                    wgpu::ShaderStages::FRAGMENT,
                    wgpu::SamplerBindingType::Filtering,
                ),
            ],
        );
        let view = |t: &wgpu::Texture| {
            t.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2Array),
                ..Default::default()
            })
        };
        let (diffuse_view, aux_view) = (view(&diffuse), view(&aux));
        let bind = desc::bind_group(
            device,
            "modern material arrays",
            &layout,
            &[
                (0, wgpu::BindingResource::TextureView(&diffuse_view)),
                (1, wgpu::BindingResource::TextureView(&aux_view)),
                (2, wgpu::BindingResource::Sampler(sampler)),
            ],
        );
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
            desc::write_mips(queue, &arrays.diffuse, 1, &levels);
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
            desc::write_mips(queue, &self.diffuse, layer, &tiled(diffuse, 4));
            if let Some(aux) = aux {
                desc::write_mips(queue, &self.aux, layer, &tiled(aux, 1));
            }
        } else {
            desc::write_mips(queue, &self.diffuse, layer, diffuse);
            if let Some(aux) = aux {
                desc::write_mips(queue, &self.aux, layer, aux);
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
