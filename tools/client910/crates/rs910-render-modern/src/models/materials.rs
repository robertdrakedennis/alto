//! The modern renderer's material cache (renderer plan M1, M2), by material
//! id.
//!
//! # RT5 materials (M1)
//!
//! The diffuse texture (archive 53 PNG, uploaded as sRGB with a mip chain,
//! so the forward pass samples linear albedo), the aux map (the HDR scale
//! the model and terrain shaders apply as `HDRScale(rgb * (1 + ratio *
//! 31))`; the same formula as the classic diffuse + aux combine, which the
//! faithful path does not use) and the material's shading parameters.
//!
//! # RT7 materials (M2)
//!
//! A version-1 material ([`Rt7MaterialExtra`], `rs910_config::nxt`) names
//! up to three maps with a size code `k` each: the diffuse (`0x20`), the
//! normal map (`0x40`) and the compound map (`0x80`). They load from the
//! block-compressed archives with their prebuilt mips (no generation):
//!
//! - **Source** ([`TextureSource`]): archive 52 (DDS `DXT5` = BC3) when the
//!   device has `TEXTURE_COMPRESSION_BC`, else archive 55 (KTX ETC2 RGBA8)
//!   when it has `TEXTURE_COMPRESSION_ETC2`, else archive 54 (PNG mip
//!   chains, RGBA8); a map that fails falls back to 54, then to 53 (the M1
//!   path, mips generated). The modern client reads DXT or PNG by the
//!   device's compression support (its ETC path is not in the Linux build).
//!   Every RT7 reference is a single (version-1) `DXT5`
//!   texture in the 910 pack (`rt7_texture_refs_are_single_dxt5_atlases`),
//!   so no material needs the DDS cubemap path.
//! - **Atlas gutter**: archives 52/54/55 store each texture at `64 << k`
//!   plus a 32-pixel gutter per side filled with the wrapped texture
//!   (proven, `nxt-data-formats.md` §6; the gutter equals the opposite
//!   edge, `atlas_gutters_hold_the_wrapped_texture`). The shader wraps (or
//!   clamps) the UV itself and maps it into the inner square
//!   ([`AtlasMeta`]), and limits the LOD to the last level whose gutter is
//!   still a texel wide. This is the shape of the modern client's atlas
//!   lookup, which has repeat, clamp and dynamic lookup modes, a mip-level
//!   count and limit, and sRGB sampling; each model batch carries a texture
//!   meta with the atlas slot and size.
//! - **Colour space**: the diffuse is sRGB with the classic texture gamma
//!   0.7 applied as the modern client applies it to DXT data: to each
//!   block's two colour endpoints (it skips this for texture type 1, the
//!   normal maps its water loads). The ETC diffuse is
//!   sampled as sRGB and gets the curve in the shader; PNG texels get it on
//!   the CPU (the M1 lookup table). Normal and compound maps are linear.
//! - **Normal map** (`0x40`): `DXT5nm`-style, X in alpha (in red in the
//!   ETC copies, [`ATLAS_X_IN_RED`]) and Y in green, Z rebuilt (the modern
//!   client's decode of its DXT normal maps: alpha and green as `rg * 2 - 1`,
//!   `b = sqrt(1 - r² - g²)`). In the 910 maps red and blue are 0 in the DXT copies and the
//!   decoded X/Y average 0 (no bias). Green points up the image in 40 of 53
//!   sampled maps (the curl of the decoded field; 11 the other way), so +Y
//!   is -V ([`crate::shaders`]). The per-material float read with the
//!   normal map (`normal_param`, 32.0 in 498 of 533) has no known meaning:
//!   **flagged**, the map applies at full strength ([`NORMAL_STRENGTH`]).
//! - **Compound map** (`0x80`): loaded and bound, **unused and flagged**:
//!   the classic renderer has no such map and the data prove no channel
//!   role (blue and alpha are 255, red and green vary).
//! - **Scalars**: the RT7 floats (the flag bits from `0x1000` up, and
//!   `0x800`) are
//!   unknown and unused; the fields RT7 shares with RT5 (alpha mode and
//!   threshold, repeat, scroll speeds) keep the M1 meaning. Opaque materials
//!   keep the classic black-is-transparent cutout, computed from
//!   the filtered colour in the shader for atlas textures.
//!
//! Materials come from the snapshot's `MaterialStore`; textures from its
//! pack. A material that does not load draws with the white texture (the
//! faithful texture provider does the same).

use std::collections::HashMap;

use rs910_config::nxt::material::{Rt7MaterialExtra, Rt7MaterialExtraStore, Rt7TextureRef};

use crate::cache::Pack;
use crate::models::material_arrays::{slot_bits, MaterialArrays};
use crate::texture::{AlphaMode, Material, MaterialStore};

/// Shading parameters of one material, packed into each draw's instance
/// record (`crate::frame::Instance`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MaterialInfo {
    /// Alpha-test reference (`AlphaTested` materials: `alphaThreshold /
    /// 255`), or 0.5 for the black-is-transparent cutout of opaque textures;
    /// negative: no cutout.
    pub alpha_ref: f32,
    /// [`FLAG_ALPHA_IS_MASK`] etc.
    pub flags: u32,
    /// `RunescapeLegacyBRDF`'s `specPower` (0: no specular term).
    pub spec_power: f32,
    /// Specular strength.
    pub spec_strength: f32,
    /// UV scroll per second (the material's `speedU/V`).
    pub scroll: [f32; 2],
}

/// The texture alpha is a specular mask, not coverage (reflective and
/// environment-mapped materials, effects 1 and 7; the classic shader forces
/// their diffuse alpha to 1).
pub const FLAG_ALPHA_IS_MASK: u32 = 1;
/// The material has an aux (HDR scale) map.
pub const FLAG_AUX: u32 = 2;

/// The strength the normal map applies with. Neutral default: the RT7
/// `normal_param` float may be a strength or a scale, but nothing proves
/// its meaning (see the module docs).
pub const NORMAL_STRENGTH: f32 = 1.0;

/// The device features the M2 texture path uses when the adapter has them
/// (`gpu_device::Device::new_with_features`).
#[must_use]
pub fn optional_device_features() -> wgpu::Features {
    wgpu::Features::TEXTURE_COMPRESSION_BC | wgpu::Features::TEXTURE_COMPRESSION_ETC2
}

/// Where an RT7 map's texels come from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextureSource {
    /// Archive 52, DDS `DXT5`, uploaded as BC3.
    Bc,
    /// Archive 55, KTX ETC2 RGBA8 (EAC).
    Etc,
    /// Archive 54, PNG mip chains.
    PngMipped,
    /// Archive 53, PNG without mips or gutter (the M1 path; last resort).
    Png,
}

impl TextureSource {
    /// The preferred source for a device with `features` (see the module
    /// docs), or the one `CLIENT910_MODERN_TEXTURES` (`bc`, `etc`, `png`) names
    /// when the device can sample it.
    #[must_use]
    pub fn for_device(features: wgpu::Features) -> Self {
        let bc = features.contains(wgpu::Features::TEXTURE_COMPRESSION_BC);
        let etc = features.contains(wgpu::Features::TEXTURE_COMPRESSION_ETC2);
        match crate::modern_debug_flags::flags().textures.as_deref() {
            Some("bc") if bc => return Self::Bc,
            Some("etc") if etc => return Self::Etc,
            Some("png") => return Self::PngMipped,
            _ => {}
        }
        if bc {
            Self::Bc
        } else if etc {
            Self::Etc
        } else {
            Self::PngMipped
        }
    }

    /// The sources tried for a map, in order, starting at `self`.
    fn chain(self) -> &'static [Self] {
        match self {
            Self::Bc => &[Self::Bc, Self::PngMipped, Self::Png],
            Self::Etc => &[Self::Etc, Self::PngMipped, Self::Png],
            Self::PngMipped => &[Self::PngMipped, Self::Png],
            Self::Png => &[Self::Png],
        }
    }
}

/// What an RT7 map holds (its colour space and gamma).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKind {
    /// Flag `0x20`: albedo (sRGB, classic texture gamma).
    Diffuse,
    /// Flag `0x40`: tangent-space normal, X in alpha, Y in green (linear).
    Normal,
    /// Flag `0x80`: compound map (linear; channel roles unknown).
    Compound,
}

/// The atlas transform of one map (the per-material atlas meta of the
/// modern shaders): `inner uv = wrap(uv) * scale + offset`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AtlasMeta {
    /// The inner square's share of the texture width: `S / (S + 2g)`.
    pub scale: f32,
    /// The gutter's share: `g / (S + 2g)`.
    pub offset: f32,
    /// The last mip level whose gutter is at least one texel.
    pub mip_limit: f32,
    /// 0: not bound; 1: bound and looked up through this transform;
    /// [`ATLAS_X_IN_RED`]: the same, and a normal map's X is in red.
    pub enabled: f32,
}

/// [`AtlasMeta::enabled`] of an ETC normal or compound map: the ETC copies
/// (archive 55) move the normal map's X from alpha to red (their alpha is 0
/// and red, green and blue are the PNG copy's alpha, green and blue;
/// `rt7_maps_upload_and_decode_like_the_png_copies`), so the shader reads X
/// from red. The ETC compound maps swap green and alpha the same way; lane Q-FID's shader reads their roughness from
/// alpha.
pub const ATLAS_X_IN_RED: f32 = 2.0;

impl AtlasMeta {
    /// The transform of a `width`-wide texture whose inner square is
    /// `inner` pixels, with `levels` mips.
    #[must_use]
    pub fn new(width: u32, inner: u32, levels: u32) -> Self {
        let gutter = width.saturating_sub(inner) / 2;
        // The gutter halves per level: level L keeps `gutter >> L` texels.
        let gutter_levels = if gutter == 0 { 0 } else { gutter.ilog2() };
        let top = levels.saturating_sub(1);
        Self {
            scale: inner as f32 / width as f32,
            offset: gutter as f32 / width as f32,
            mip_limit: if gutter == 0 {
                top
            } else {
                top.min(gutter_levels)
            } as f32,
            enabled: 1.0,
        }
    }
}

/// The `MaterialMeta` uniform block of [`crate::shaders::FORWARD_WGSL`]
/// (group 1, binding 5).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialMeta {
    pub diffuse: AtlasMeta,
    pub normal: AtlasMeta,
    /// Bound, not sampled (see the module docs).
    pub compound: AtlasMeta,
    /// x: 1 = black texels are transparent (the classic cutout from the
    /// filtered colour); y: 1 = apply the classic texture gamma in the shader
    /// (ETC diffuse); z: normal strength; w: repeat bits (1 U, 2 V).
    pub params: [f32; 4],
}

/// One mip level: width, height, bytes (whole 4x4 blocks for BC/ETC).
pub type Level = (u32, u32, Vec<u8>);

/// One RT7 map decoded on the CPU, ready to upload.
#[derive(Clone, Debug, PartialEq)]
pub struct MapData {
    pub source: TextureSource,
    pub format: wgpu::TextureFormat,
    pub levels: Vec<Level>,
    pub meta: AtlasMeta,
}

/// What one material resolved to (tests, stats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MaterialResolution {
    /// A version-1 material with an extras record.
    pub rt7: bool,
    /// The source each referenced map loaded from (`None`: not referenced
    /// or not loaded).
    pub diffuse: Option<TextureSource>,
    pub normal: Option<TextureSource>,
    pub compound: Option<TextureSource>,
    /// Referenced maps that did not load from any source.
    pub missing: u8,
}

/// One cached material.
pub struct MaterialEntry {
    /// The material's own textures (group 1): what a draw without a layer
    /// of the arrays ([`slot_bits`](Self::slot_bits) zero), a billboard or a
    /// particle binds.
    pub bind_group: wgpu::BindGroup,
    /// The draw's flags bits that name the material's layer of the
    /// [`MaterialArrays`] (0: the material draws with its own textures).
    pub slot_bits: u32,
    /// Its meta block (group 1) is not the zero block: an RT7 material's
    /// atlas transforms, which the shaders branch on. A draw that reads
    /// the layers of the arrays needs group 1 to hold a zero block.
    pub atlas: bool,
    pub info: MaterialInfo,
    pub resolution: MaterialResolution,
}

/// Counters over the cached materials (logged with the frame stats).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextureStats {
    /// RT7 materials cached.
    pub rt7: usize,
    /// RT7 maps loaded, by kind.
    pub normal_maps: usize,
    pub compound_maps: usize,
    /// RT7 maps loaded from a fallback source.
    pub fallbacks: usize,
    /// Referenced RT7 maps that did not load.
    pub missing: usize,
}

/// See the module docs.
pub struct Textures {
    pub layout: wgpu::BindGroupLayout,
    entries: crate::fast_hash::FastMap<i32, MaterialEntry>,
    /// The RT5 maps as texture arrays (group 4 of the model pipelines).
    pub arrays: MaterialArrays,
    /// Whether material `id - 1` has a layer, by index (the draw's hot path
    /// asks this per draw).
    arrayed: Vec<bool>,
    /// Group 1 of a pass that draws only array materials: nothing in it is
    /// read, it is bound because the pipeline layout has the group.
    neutral: wgpu::BindGroup,
    white: wgpu::TextureView,
    no_aux: wgpu::TextureView,
    /// A flat normal (X = Y = 0 in the alpha/green encoding).
    flat_normal: wgpu::TextureView,
    /// The compound map of materials without one.
    no_compound: wgpu::TextureView,
    /// Samplers by `(repeat_s, repeat_t)`.
    samplers: [wgpu::Sampler; 4],
    /// The RT7 source this device prefers.
    source: TextureSource,
    /// Draw RT7 materials as M1 did (the RT5 path: archive 53, no normal
    /// or compound map); verification only ([`Textures::set_rt7_source`]).
    rt7_as_rt5: bool,
    /// The RT7 side table, loaded from the first pack seen (`None` inside:
    /// it failed to load; every material is then drawn as RT5).
    rt7: Option<Option<Rt7MaterialExtraStore>>,
    /// Maps decoded ahead of their first use ([`Textures::prefetch`]).
    prefetched: HashMap<i32, Decoded>,
    /// Materials whose texture failed to load (drawn white), for the log.
    pub failed: usize,
    pub stats: TextureStats,
    /// Which RT5 materials take the environment map (the settings').
    pub env_reflections: crate::settings::EnvReflections,
}

/// Upload `levels` (level 0 first) as one 2D texture. Block formats take
/// whole blocks per level; a level smaller than a block is copied at the
/// block's size (wgpu's physical mip size).
pub(crate) fn upload(
    device: &wgpu::Device,
    queue: &dyn rs910_gpu_device::uploads::Uploader,
    label: &str,
    format: wgpu::TextureFormat,
    levels: &[Level],
) -> wgpu::TextureView {
    let (w, h, _) = &levels[0];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: *w,
            height: *h,
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let (bw, bh) = format.block_dimensions();
    let block_bytes = format.block_copy_size(None).unwrap_or(4);
    for (level, (w, h, data)) in levels.iter().enumerate() {
        let (blocks_x, blocks_y) = (w.div_ceil(bw), h.div_ceil(bh));
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(blocks_x * block_bytes),
                rows_per_image: Some(blocks_y),
            },
            wgpu::Extent3d {
                width: blocks_x * bw,
                height: blocks_y * bh,
                depth_or_array_layers: 1,
            },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

/// A box-filtered mip chain of `channels`-byte texels (`w`, `h` powers of
/// two or not; each level halves, rounding down, to 1x1). RGB channels are
/// averaged in linear light (the texture is sampled as sRGB), alpha
/// linearly.
pub(crate) fn mip_chain(w: u32, h: u32, channels: usize, srgb: bool, px: Vec<u8>) -> Vec<Level> {
    let to_linear: Vec<f32> = (0..256)
        .map(|v| crate::post::tonemap::srgb_to_linear(v as f32 / 255.0))
        .collect();
    let mut levels = vec![(w, h, px)];
    loop {
        let (pw, ph, prev) = levels.last().expect("level");
        if *pw == 1 && *ph == 1 {
            break;
        }
        let (nw, nh) = ((pw / 2).max(1), (ph / 2).max(1));
        let mut next = vec![0_u8; (nw * nh) as usize * channels];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..channels {
                    let mut sum = 0.0;
                    let mut n = 0.0;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(pw - 1);
                        let sy = (y * 2 + dy).min(ph - 1);
                        let v = prev[((sy * pw + sx) as usize) * channels + c];
                        sum += if srgb && c < 3 {
                            to_linear[v as usize]
                        } else {
                            f32::from(v) / 255.0
                        };
                        n += 1.0;
                    }
                    let avg = sum / n;
                    let out = if srgb && c < 3 {
                        crate::post::tonemap::linear_to_srgb(avg)
                    } else {
                        avg
                    };
                    next[((y * nw + x) as usize) * channels + c] =
                        (out * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
        levels.push((nw, nh, next));
    }
    levels
}

/// The classic texture gamma (0.7 on RGB): `floor((i / 255)^0.7 * 255)`.
pub(crate) fn gamma_lut() -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        *slot = ((i as f64 / 255.0).powf(0.7) * 255.0).min(255.0) as u8;
    }
    lut
}

/// Apply `lut` to the two colour endpoints of every BC3 block (RGB565,
/// each channel widened to 8 bits, mapped, and rounded back). The modern
/// client does the same to its DXT colour textures at load. A BC3 colour block always decodes in four-colour
/// mode, so the endpoints' order does not matter and no index changes.
pub fn gamma_bc3_endpoints(blocks: &mut [u8], lut: &[u8; 256]) {
    let map = |v: u16| -> u16 {
        let r5 = (v >> 11) & 0x1f;
        let g6 = (v >> 5) & 0x3f;
        let b5 = v & 0x1f;
        let widen5 = |c: u16| usize::from((c << 3) | (c >> 2));
        let widen6 = |c: u16| usize::from((c << 2) | (c >> 4));
        let narrow = |c: u8, max: u32| ((u32::from(c) * max + 127) / 255) as u16;
        let r = narrow(lut[widen5(r5)], 31);
        let g = narrow(lut[widen6(g6)], 63);
        let b = narrow(lut[widen5(b5)], 31);
        (r << 11) | (g << 5) | b
    };
    for block in blocks.chunks_exact_mut(16) {
        for at in [8, 10] {
            let v = map(u16::from_le_bytes([block[at], block[at + 1]]));
            block[at..at + 2].copy_from_slice(&v.to_le_bytes());
        }
    }
}

fn le32(bytes: &[u8], at: usize) -> anyhow::Result<u32> {
    let word = bytes
        .get(at..at + 4)
        .ok_or_else(|| anyhow::anyhow!("truncated header at {at}"))?;
    Ok(u32::from_le_bytes([word[0], word[1], word[2], word[3]]))
}

/// Bytes of a `w x h` level in 16-byte 4x4 blocks.
fn block_level_bytes(w: u32, h: u32) -> usize {
    (w.div_ceil(4) * h.div_ceil(4)) as usize * 16
}

/// The mip levels of a single-face `DXT5` DDS (the data after the 128-byte
/// header, level by level).
fn dds_levels(dds: &[u8]) -> anyhow::Result<(u32, Vec<Level>)> {
    anyhow::ensure!(dds.get(84..88) == Some(&b"DXT5"[..]), "not a DXT5 DDS");
    anyhow::ensure!(le32(dds, 112)? & 0xFE00 == 0, "a cubemap DDS");
    let h = le32(dds, 12)?;
    let w = le32(dds, 16)?;
    let mips = if le32(dds, 8)? & 0x2_0000 != 0 {
        le32(dds, 28)?.max(1)
    } else {
        1
    };
    let mut at = 128;
    let mut levels = Vec::with_capacity(mips as usize);
    for level in 0..mips {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        let n = block_level_bytes(lw, lh);
        let data = dds
            .get(at..at + n)
            .ok_or_else(|| anyhow::anyhow!("DDS level {level} runs past the end"))?;
        levels.push((lw, lh, data.to_vec()));
        at += n;
    }
    anyhow::ensure!(
        at == dds.len(),
        "{} bytes after the last DDS level",
        dds.len() - at
    );
    Ok((w, levels))
}

/// The mip levels of a single-face ETC2 RGBA8 KTX 1.1 file.
fn ktx_levels(ktx: &[u8]) -> anyhow::Result<(u32, Vec<Level>)> {
    anyhow::ensure!(le32(ktx, 28)? == 0x9278, "not an ETC2 RGBA8 KTX");
    anyhow::ensure!(le32(ktx, 52)? == 1, "a cubemap KTX");
    let w = le32(ktx, 36)?;
    let h = le32(ktx, 40)?;
    let mips = le32(ktx, 56)?.max(1);
    let mut at = 64 + le32(ktx, 60)? as usize;
    let mut levels = Vec::with_capacity(mips as usize);
    for level in 0..mips {
        let (lw, lh) = ((w >> level).max(1), (h >> level).max(1));
        let n = le32(ktx, at)? as usize;
        anyhow::ensure!(
            n == block_level_bytes(lw, lh),
            "KTX level {level} is {n} bytes"
        );
        let data = ktx
            .get(at + 4..at + 4 + n)
            .ok_or_else(|| anyhow::anyhow!("KTX level {level} runs past the end"))?;
        levels.push((lw, lh, data.to_vec()));
        at += 4 + n.next_multiple_of(4);
    }
    anyhow::ensure!(
        at == ktx.len(),
        "{} bytes after the last KTX level",
        ktx.len() - at
    );
    Ok((w, levels))
}

/// Check that `levels` is a full chain from a square `width`: each level
/// half the last, down to 1x1.
fn check_chain(width: u32, levels: &[Level]) -> anyhow::Result<()> {
    anyhow::ensure!(!levels.is_empty(), "no mip levels");
    anyhow::ensure!(levels[0].1 == width, "not square");
    for (i, (w, h, _)) in levels.iter().enumerate() {
        let want = (width >> i).max(1);
        anyhow::ensure!((*w, *h) == (want, want), "level {i} is {w}x{h}");
    }
    anyhow::ensure!(
        levels.len() as u32 == width.ilog2() + 1,
        "{} levels for width {width}",
        levels.len()
    );
    Ok(())
}

/// Decode map `r` of `kind` from `source` on the CPU (see the module docs;
/// no GPU needed, so the structural tests call it).
pub fn load_map(
    pack: &Pack,
    source: TextureSource,
    kind: MapKind,
    r: Rt7TextureRef,
) -> anyhow::Result<MapData> {
    anyhow::ensure!(r.texture >= 0, "texture id {}", r.texture);
    let id = r.texture as u32;
    let inner = r.size();
    let srgb = kind == MapKind::Diffuse;
    let lut = gamma_lut();
    let one_face = |faces: usize| -> anyhow::Result<()> {
        anyhow::ensure!(faces == 1, "texture {id} has {faces} faces");
        Ok(())
    };
    let (format, width, levels) = match source {
        TextureSource::Bc => {
            let faces = crate::texture::load_dxt(pack, id, 0)?;
            one_face(faces.len())?;
            let (width, mut levels) = dds_levels(&faces[0].bytes)?;
            if srgb {
                for (_, _, data) in &mut levels {
                    gamma_bc3_endpoints(data, &lut);
                }
            }
            let format = if srgb {
                wgpu::TextureFormat::Bc3RgbaUnormSrgb
            } else {
                wgpu::TextureFormat::Bc3RgbaUnorm
            };
            (format, width, levels)
        }
        TextureSource::Etc => {
            let faces = crate::texture::load_etc(pack, id, 0)?;
            one_face(faces.len())?;
            let (width, levels) = ktx_levels(&faces[0].bytes)?;
            // The gamma is the shader's (`MaterialMeta.params.y`).
            let format = if srgb {
                wgpu::TextureFormat::Etc2Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Etc2Rgba8Unorm
            };
            (format, width, levels)
        }
        TextureSource::PngMipped => {
            let mut mipped = crate::texture::load_mipped(pack, id, 0)?;
            one_face(mipped.faces.len())?;
            let images = mipped.faces.remove(0);
            let width = images.first().map_or(0, |i| i.w);
            let levels = images
                .into_iter()
                .map(|mut image| {
                    image.px.truncate((image.w * image.h * 4) as usize);
                    if srgb {
                        for p in image.px.chunks_exact_mut(4) {
                            for c in &mut p[..3] {
                                *c = lut[usize::from(*c)];
                            }
                        }
                    }
                    (image.w, image.h, image.px)
                })
                .collect();
            let format = if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            (format, width, levels)
        }
        TextureSource::Png => {
            // The source size, no gutter; mips generated (M1).
            let mut image = crate::texture::load_png(pack, id, 0)?;
            anyhow::ensure!(
                image.w == image.h && image.w.is_power_of_two(),
                "texture {id}: {}x{} source",
                image.w,
                image.h
            );
            image.px.truncate((image.w * image.h * 4) as usize);
            if srgb {
                for p in image.px.chunks_exact_mut(4) {
                    for c in &mut p[..3] {
                        *c = lut[usize::from(*c)];
                    }
                }
            }
            let levels = mip_chain(image.w, image.h, 4, srgb, image.px);
            let format = if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            };
            let meta = AtlasMeta::new(image.w, image.w, levels.len() as u32);
            return Ok(MapData {
                source,
                format,
                levels,
                meta,
            });
        }
    };
    check_chain(width, &levels)?;
    anyhow::ensure!(
        width == inner + 64,
        "texture {id} is {width} wide, size code {} wants {} + 64",
        r.size_code,
        inner
    );
    let mut meta = AtlasMeta::new(width, inner, levels.len() as u32);
    // Also marks the ETC compound maps (their roughness is in
    // alpha; the pre-lane shader does not sample them).
    if source == TextureSource::Etc && matches!(kind, MapKind::Normal | MapKind::Compound) {
        meta.enabled = ATLAS_X_IN_RED;
    }
    Ok(MapData {
        source,
        format,
        levels,
        meta,
    })
}

/// Map `r` from the first source of `preferred`'s chain that loads.
pub fn load_map_with_fallback(
    pack: &Pack,
    preferred: TextureSource,
    kind: MapKind,
    r: Rt7TextureRef,
) -> anyhow::Result<MapData> {
    let mut errors = Vec::new();
    for &source in preferred.chain() {
        match load_map(pack, source, kind, r) {
            Ok(map) => return Ok(map),
            Err(error) => errors.push(format!("{source:?}: {error:#}")),
        }
    }
    anyhow::bail!("texture {}: {}", r.texture, errors.join("; "))
}

/// The shading parameters of `material` (`None`: the untextured white
/// material, id -1).
#[must_use]
pub fn material_info(material: Option<&Material>) -> MaterialInfo {
    let Some(m) = material else {
        return MaterialInfo {
            alpha_ref: -1.0,
            ..MaterialInfo::default()
        };
    };
    let reflective = matches!(m.effect, 1 | 7);
    let water = matches!(m.effect, 2 | 4 | 8 | 9);
    let mut info = MaterialInfo {
        alpha_ref: match m.alpha {
            AlphaMode::AlphaTested if m.alpha_threshold != 0 => {
                f32::from(m.alpha_threshold) / 255.0
            }
            // An opaque, non-reflective
            // texture's black texels are transparent.
            AlphaMode::None if !reflective => 0.5,
            _ => -1.0,
        },
        flags: 0,
        spec_power: 0.0,
        spec_strength: 0.0,
        scroll: [m.speed_u, m.speed_v],
    };
    // The classic unlit program for effect 6 (atmosphere::fog fix 3).
    info.flags |= crate::atmosphere::fog::material_flags(Some(m));
    if reflective {
        info.flags |= FLAG_ALPHA_IS_MASK;
        // `MaterialState.material`: the effect argument chooses the
        // exponent (1 -> 32, 2 -> 4, 3 -> 1).
        info.spec_power = match m.effect_param {
            2 => 4.0,
            3 => 1.0,
            _ => 32.0,
        };
        info.spec_strength = 1.0;
    } else if water {
        info.spec_power = 48.0;
        info.spec_strength = 0.6;
    }
    info
}

impl Textures {
    pub fn new(device: &wgpu::Device, queue: &dyn rs910_gpu_device::uploads::Uploader) -> Self {
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("modern material"),
            entries: &[
                texture_entry(0),
                texture_entry(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(3),
                texture_entry(4),
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(
                            std::mem::size_of::<MaterialMeta>() as u64,
                        ),
                    },
                    count: None,
                },
            ],
        });
        let white = upload(
            device,
            queue,
            "modern white",
            wgpu::TextureFormat::Rgba8UnormSrgb,
            &[(1, 1, vec![255; 4])],
        );
        let no_aux = upload(
            device,
            queue,
            "modern no aux",
            wgpu::TextureFormat::R8Unorm,
            &[(1, 1, vec![0])],
        );
        let flat_normal = upload(
            device,
            queue,
            "modern flat normal",
            wgpu::TextureFormat::Rgba8Unorm,
            &[(1, 1, vec![0, 128, 0, 128])],
        );
        let no_compound = upload(
            device,
            queue,
            "modern no compound",
            wgpu::TextureFormat::Rgba8Unorm,
            &[(1, 1, vec![0, 0, 255, 255])],
        );
        let sampler = |s: bool, t: bool| {
            let mode = |r: bool| {
                if r {
                    wgpu::AddressMode::Repeat
                } else {
                    wgpu::AddressMode::ClampToEdge
                }
            };
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("modern material"),
                address_mode_u: mode(s),
                address_mode_v: mode(t),
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                // The forward shader blends the two mip levels itself
                // (`sample_trilinear`, `sample_atlas`): the hardware's
                // linear mip filter is not deterministic run to run on the
                // Apple GPUs (a few texels per frame flip between two
                // results).
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            })
        };
        let source = TextureSource::for_device(device.features());
        log::info!("[modern] RT7 material textures: {source:?} preferred");
        let samplers = [
            sampler(false, false),
            sampler(true, false),
            sampler(false, true),
            sampler(true, true),
        ];
        let arrays = MaterialArrays::new(device, queue, &samplers[3]);
        let neutral_meta = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("modern neutral material meta"),
                contents: bytemuck::bytes_of(&MaterialMeta::default()),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        let neutral = {
            let view = |binding, view| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            };
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("modern neutral material"),
                layout: &layout,
                entries: &[
                    view(0, &white),
                    view(1, &no_aux),
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&samplers[3]),
                    },
                    view(3, &flat_normal),
                    view(4, &no_compound),
                    wgpu::BindGroupEntry {
                        binding: 5,
                        resource: neutral_meta.as_entire_binding(),
                    },
                ],
            })
        };
        Self {
            layout,
            entries: Default::default(),
            arrays,
            arrayed: Vec::new(),
            neutral,
            white,
            no_aux,
            flat_normal,
            no_compound,
            samplers,
            source,
            rt7_as_rt5: false,
            rt7: None,
            prefetched: HashMap::new(),
            failed: 0,
            stats: TextureStats::default(),
            env_reflections: crate::settings::EnvReflections::Proven,
        }
    }

    /// Materials cached so far.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether no material is cached yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The RT7 source this device prefers.
    #[must_use]
    pub fn source(&self) -> TextureSource {
        self.source
    }

    /// Draw RT7 materials from `source` (`None`: as M1 did, through the
    /// RT5 path), dropping every cached material. Verification only (the
    /// source comparison frames); the renderer picks
    /// [`TextureSource::for_device`].
    pub fn set_rt7_source(&mut self, source: Option<TextureSource>) {
        self.entries.clear();
        self.arrayed.clear();
        self.arrays.reset();
        self.stats = TextureStats::default();
        self.failed = 0;
        self.rt7_as_rt5 = source.is_none();
        if let Some(source) = source {
            self.source = source;
        }
    }

    /// The RT7 extras of material `id` (loading the side table from `pack`
    /// on first use).
    fn rt7_extra(&mut self, pack: &Pack, id: i32) -> Option<Rt7MaterialExtra> {
        if self.rt7_as_rt5 {
            return None;
        }
        let store = self
            .rt7
            .get_or_insert_with(|| match Rt7MaterialExtraStore::load(pack) {
                Ok(store) => {
                    log::info!("[modern] {} RT7 material extras loaded", store.len());
                    Some(store)
                }
                Err(error) => {
                    log::warn!("[modern] RT7 material extras: {error:#}; RT7 drawn as RT5");
                    None
                }
            });
        let id = u32::try_from(id).ok()?;
        store.as_ref()?.get(id).cloned()
    }

    /// Load `id` (`-1`: white) on first use: its maps decoded by
    /// [`Self::prefetch`] when that ran for it, else here, then uploaded.
    pub fn ensure(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        pack: Option<&Pack>,
        materials: Option<&MaterialStore>,
        id: i32,
    ) {
        if self.entries.contains_key(&id) {
            return;
        }
        let decoded = match self.prefetched.remove(&id) {
            Some(decoded) => decoded,
            None => self.job(pack, materials, id).decode(pack),
        };
        self.install(device, queue, pack, materials, id, decoded);
    }

    /// Decode the maps of the materials of `ids` not cached yet, with
    /// `decode` running [`MaterialJob::decode`] over the jobs in order (the
    /// renderer's threads, performance plan P5); [`Self::ensure`] uploads
    /// each at its first use, as it would have loaded it there.
    pub(crate) fn prefetch<'m>(
        &mut self,
        pack: Option<&Pack>,
        materials: Option<&'m MaterialStore>,
        ids: impl IntoIterator<Item = i32>,
        decode: impl FnOnce(&[MaterialJob<'m>]) -> Vec<Decoded>,
    ) {
        let mut seen = std::collections::HashSet::new();
        let jobs: Vec<MaterialJob<'m>> = ids
            .into_iter()
            .filter(|id| {
                !self.entries.contains_key(id)
                    && !self.prefetched.contains_key(id)
                    && seen.insert(*id)
            })
            .collect::<Vec<_>>()
            .into_iter()
            .map(|id| self.job(pack, materials, id))
            .collect();
        if jobs.is_empty() {
            return;
        }
        let decoded = decode(&jobs);
        for (job, decoded) in jobs.iter().zip(decoded) {
            self.prefetched.insert(job.id, decoded);
        }
    }

    /// Drop the decoded maps no draw used (the frame's end).
    pub(crate) fn clear_prefetched(&mut self) {
        self.prefetched.clear();
    }

    /// What decoding material `id` needs (its RT7 extras found here, on the
    /// calling thread: the side table is this cache's).
    pub(crate) fn job<'m>(
        &mut self,
        pack: Option<&Pack>,
        materials: Option<&'m MaterialStore>,
        id: i32,
    ) -> MaterialJob<'m> {
        let material = (id >= 0)
            .then(|| materials.and_then(|m| m.get(id as u32)))
            .flatten();
        let extra = match (material, pack) {
            (Some(_), Some(pack)) => self.rt7_extra(pack, id),
            _ => None,
        };
        MaterialJob {
            id,
            material,
            extra,
            source: self.source,
        }
    }

    /// Upload `decoded`, material `id`'s maps, and cache the material.
    fn install(
        &mut self,
        device: &wgpu::Device,
        queue: &dyn rs910_gpu_device::uploads::Uploader,
        pack: Option<&Pack>,
        materials: Option<&MaterialStore>,
        id: i32,
        decoded: Decoded,
    ) {
        let material = (id >= 0)
            .then(|| materials.and_then(|m| m.get(id as u32)))
            .flatten();
        let mut info = material_info(material);
        // The modern client's RT5 vertex flags (the specular map, the
        // environment map; `lighting::env_reflections`).
        if let Some(m) = material {
            info.flags |= crate::lighting::env_reflections::material_flags(m, self.env_reflections);
        }
        let mut diffuse = None;
        let mut aux = None;
        let mut normal = None;
        let mut compound = None;
        let mut meta = MaterialMeta::default();
        let mut resolution = MaterialResolution::default();
        // The layer of the arrays an RT5 material's maps were written into.
        let mut placed: Option<(u32, bool)> = None;
        let repeat = material.map_or(3, |m| {
            usize::from(m.repeat_s == 1) | usize::from(m.repeat_t == 1) << 1
        });
        match (material, pack, decoded) {
            (Some(m), Some(_), Decoded::Rt7 { maps }) => {
                resolution.rt7 = true;
                self.stats.rt7 += 1;
                let [d, n, c] = maps.map(|(kind, map)| {
                    let map = map?;
                    match map {
                        Ok(map) => {
                            if map.source != self.source {
                                self.stats.fallbacks += 1;
                            }
                            Some(map)
                        }
                        Err(error) => {
                            resolution.missing += 1;
                            self.stats.missing += 1;
                            log::warn!("[modern] material {id} {kind:?} map: {error:#}");
                            None
                        }
                    }
                });
                let reflective = matches!(m.effect, 1 | 7);
                if let Some(map) = d {
                    resolution.diffuse = Some(map.source);
                    meta.diffuse = map.meta;
                    // The classic black-is-transparent cutout of opaque textures,
                    // from the filtered colour (the alpha is the texture's).
                    meta.params[0] = f32::from(m.alpha == AlphaMode::None && !reflective);
                    meta.params[1] = f32::from(map.source == TextureSource::Etc);
                    diffuse = Some(upload(
                        device,
                        queue,
                        "modern rt7 diffuse",
                        map.format,
                        &map.levels,
                    ));
                } else {
                    self.failed += 1;
                    info.alpha_ref = -1.0;
                }
                if let Some(map) = n {
                    resolution.normal = Some(map.source);
                    self.stats.normal_maps += 1;
                    meta.normal = map.meta;
                    normal = Some(upload(
                        device,
                        queue,
                        "modern rt7 normal",
                        map.format,
                        &map.levels,
                    ));
                }
                if let Some(map) = c {
                    resolution.compound = Some(map.source);
                    self.stats.compound_maps += 1;
                    meta.compound = map.meta;
                    compound = Some(upload(
                        device,
                        queue,
                        "modern rt7 compound",
                        map.format,
                        &map.levels,
                    ));
                }
                meta.params[2] = NORMAL_STRENGTH;
            }
            (Some(_), Some(_), Decoded::Rt5 { diffuse: d, aux: a }) => {
                match d {
                    Ok(levels) => {
                        // The arrays address by repeat on both axes.
                        if repeat == 3 {
                            let aux_levels = a.as_ref().and_then(|(_, r)| r.as_ref().ok());
                            placed =
                                self.arrays
                                    .place(queue, &levels, aux_levels.map(Vec::as_slice));
                        }
                        diffuse = Some(upload(
                            device,
                            queue,
                            "modern diffuse",
                            wgpu::TextureFormat::Rgba8UnormSrgb,
                            &levels,
                        ));
                    }
                    Err(error) => {
                        self.failed += 1;
                        log::debug!("[modern] material {id}: {error:#}; drawn white");
                        info.alpha_ref = -1.0;
                    }
                }
                match a {
                    Some((_, Ok(levels))) => {
                        aux = Some(upload(
                            device,
                            queue,
                            "modern aux",
                            wgpu::TextureFormat::R8Unorm,
                            &levels,
                        ));
                        info.flags |= FLAG_AUX;
                    }
                    Some((aux_id, Err(error))) => {
                        log::debug!("[modern] material {id} aux {aux_id}: {error:#}");
                    }
                    None => {}
                }
            }
            // No material, no pack: white (the white layer stands for it).
            (_, _, Decoded::None) => placed = self.arrays.white(),
            _ => {}
        }
        meta.params[3] = repeat as f32;
        let meta_buffer = wgpu::util::DeviceExt::create_buffer_init(
            device,
            &wgpu::util::BufferInitDescriptor {
                label: Some("modern material meta"),
                contents: bytemuck::bytes_of(&meta),
                usage: wgpu::BufferUsages::UNIFORM,
            },
        );
        fn view(binding: u32, view: &wgpu::TextureView) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::TextureView(view),
            }
        }
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("modern material"),
            layout: &self.layout,
            entries: &[
                view(0, diffuse.as_ref().unwrap_or(&self.white)),
                view(1, aux.as_ref().unwrap_or(&self.no_aux)),
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.samplers[repeat]),
                },
                view(3, normal.as_ref().unwrap_or(&self.flat_normal)),
                view(4, compound.as_ref().unwrap_or(&self.no_compound)),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: meta_buffer.as_entire_binding(),
                },
            ],
        });
        let slot_bits = placed.map_or(0, |(layer, tiled)| slot_bits(layer, tiled));
        // The repeat bits only matter to the atlas lookup.
        let atlas = MaterialMeta {
            params: [meta.params[0], meta.params[1], meta.params[2], 0.0],
            ..meta
        } != MaterialMeta::default();
        // The draw's hot path asks whether a material has a layer by index
        // (id -1, untextured, is index 0).
        if let Ok(index) = usize::try_from(i64::from(id) + 1) {
            if self.arrayed.len() <= index {
                self.arrayed.resize(index + 1, false);
            }
            self.arrayed[index] = slot_bits != 0;
        }
        self.entries.insert(
            id,
            MaterialEntry {
                bind_group,
                slot_bits,
                atlas,
                info,
                resolution,
            },
        );
    }

    /// Whether material `id` draws from its layer of the arrays (its draws
    /// bind no material, `frame::submit`).
    #[must_use]
    pub fn is_arrayed(&self, id: i32) -> bool {
        usize::try_from(i64::from(id) + 1)
            .ok()
            .and_then(|i| self.arrayed.get(i))
            .copied()
            .unwrap_or(false)
    }

    /// How many cached materials have a layer of the arrays, and how many
    /// draw with their own textures.
    #[must_use]
    pub fn layer_counts(&self) -> (usize, usize) {
        let arrayed = self.entries.values().filter(|e| e.slot_bits != 0).count();
        (arrayed, self.entries.len() - arrayed)
    }

    /// Group 1 for a pass whose draws are all array materials'.
    #[must_use]
    pub fn neutral(&self) -> &wgpu::BindGroup {
        &self.neutral
    }

    /// A material loaded by [`Self::ensure`].
    #[must_use]
    pub fn get(&self, id: i32) -> Option<&MaterialEntry> {
        self.entries.get(&id)
    }

    /// Every cached material's resolution, by id.
    pub fn resolutions(&self) -> impl Iterator<Item = (i32, MaterialResolution)> + '_ {
        self.entries.iter().map(|(&id, e)| (id, e.resolution))
    }
}

/// What decoding one material's maps needs ([`Textures::job`]); any thread
/// may run [`MaterialJob::decode`].
pub(crate) struct MaterialJob<'m> {
    id: i32,
    material: Option<&'m Material>,
    extra: Option<Rt7MaterialExtra>,
    source: TextureSource,
}

/// One material's maps decoded on the CPU ([`MaterialJob::decode`]), ready
/// for [`Textures::ensure`] to upload.
pub(crate) enum Decoded {
    /// A version-1 material: its diffuse, normal and compound maps, each
    /// `None` when not referenced.
    Rt7 {
        maps: [(MapKind, Option<anyhow::Result<MapData>>); 3],
    },
    /// The RT5 diffuse's mip chain and the aux map's (with its texture id).
    Rt5 {
        diffuse: anyhow::Result<Vec<Level>>,
        aux: Option<(u32, anyhow::Result<Vec<Level>>)>,
    },
    /// No material or no pack: white.
    None,
}

impl MaterialJob<'_> {
    /// Decode the maps (no cache touched; see [`Textures::prefetch`]).
    pub(crate) fn decode(&self, pack: Option<&Pack>) -> Decoded {
        match (self.material, pack, self.extra.as_ref()) {
            (Some(_), Some(pack), Some(extra)) => {
                let load = |kind: MapKind, r: Option<Rt7TextureRef>| {
                    (
                        kind,
                        r.map(|r| load_map_with_fallback(pack, self.source, kind, r)),
                    )
                };
                Decoded::Rt7 {
                    maps: [
                        load(MapKind::Diffuse, extra.diffuse),
                        load(MapKind::Normal, extra.normal),
                        load(MapKind::Compound, extra.compound),
                    ],
                }
            }
            (Some(m), Some(pack), None) => Decoded::Rt5 {
                diffuse: decode_diffuse(pack, m),
                aux: m.aux_texture.map(|aux_id| {
                    let levels = crate::texture::load_png(pack, aux_id, 0).map(|image| {
                        let r: Vec<u8> = image.px.chunks_exact(4).map(|p| p[0]).collect();
                        mip_chain(image.w, image.h, 1, false, r)
                    });
                    (aux_id, levels)
                }),
            },
            _ => Decoded::None,
        }
    }
}

/// The diffuse map of RT5 material `m` as an sRGB mip chain.
/// Opaque, non-reflective materials keep the classic black-is-transparent alpha;
/// the others keep the PNG's alpha.
fn decode_diffuse(pack: &Pack, m: &Material) -> anyhow::Result<Vec<Level>> {
    let id = m
        .diffuse_texture
        .ok_or_else(|| anyhow::anyhow!("material {} has no diffuse texture", m.id))?;
    let mut image = crate::texture::load_png(pack, id, 0)?;
    anyhow::ensure!(
        image.w > 0 && image.h > 0 && image.px.len() >= (image.w * image.h * 4) as usize,
        "texture {id}: empty or short"
    );
    let reflective = matches!(m.effect, 1 | 7);
    // The classic texture gamma (0.7 on
    // RGB): the 910 textures are authored for it, so the modern pass keeps the
    // faithful albedo and changes only the lighting.
    let lut = gamma_lut();
    let opaque = m.alpha == AlphaMode::None && !reflective;
    for p in image.px.chunks_exact_mut(4) {
        if opaque {
            p[3] = if p[0] | p[1] | p[2] == 0 { 0 } else { 255 };
        }
        for c in &mut p[..3] {
            *c = lut[usize::from(*c)];
        }
    }
    image.px.truncate((image.w * image.h * 4) as usize);
    Ok(mip_chain(image.w, image.h, 4, true, image.px))
}

#[cfg(test)]
mod tests;
