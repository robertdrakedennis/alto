//! The terrain texture array (M10): every material the scene's NXT
//! terrain names, resampled to one layer size so a triangle can blend up
//! to three of them in one draw. It is a `texture_2d_array` and a table
//! of per-layer settings, instead of one atlas image and a settings
//! texture.
//!
//! Texels are the M1 material path's (`models::materials`): the archive-53 PNG
//! with the classic texture gamma (0.7), sRGB, a box-filtered mip chain. The
//! terrain is opaque (the texture sampling forces alpha to 1), so the
//! classic black-is-transparent cutout does not apply.

use crate::cache::Pack;
use crate::texture::Material;

/// Texels per side of one layer (most 910 floor textures are 128 or 256
/// pixels, `nxt-data-formats.md` §6; smaller ones are upsampled, larger
/// ones box-filtered).
pub const LAYER_SIZE: u32 = 256;

/// One layer's RGBA8 (sRGB) mip chain, level 0 first, each level square.
pub type LayerLevels = Vec<(u32, u32, Vec<u8>)>;

/// Resample `px` (`w` x `h` RGBA8) to `size` x `size` with a wrapping
/// bilinear filter (a factor-2 reduction is the 2x2 box).
#[must_use]
pub fn resample_wrapped(w: u32, h: u32, px: &[u8], size: u32) -> Vec<u8> {
    if w == size && h == size {
        return px.to_vec();
    }
    let mut out = vec![0_u8; (size * size * 4) as usize];
    let (sw, sh) = (w as f32 / size as f32, h as f32 / size as f32);
    let texel = |x: i64, y: i64, c: usize| -> f32 {
        let x = x.rem_euclid(i64::from(w)) as usize;
        let y = y.rem_euclid(i64::from(h)) as usize;
        f32::from(px[(y * w as usize + x) * 4 + c])
    };
    for y in 0..size {
        for x in 0..size {
            let fx = (x as f32 + 0.5) * sw - 0.5;
            let fy = (y as f32 + 0.5) * sh - 0.5;
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (x0, y0) = (x0 as i64, y0 as i64);
            for c in 0..4 {
                let a = texel(x0, y0, c) * (1.0 - tx) + texel(x0 + 1, y0, c) * tx;
                let b = texel(x0, y0 + 1, c) * (1.0 - tx) + texel(x0 + 1, y0 + 1, c) * tx;
                let v = a * (1.0 - ty) + b * ty;
                out[((y * size + x) * 4) as usize + c] = v.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    out
}

/// The layer of `material`: its diffuse PNG with the classic texture gamma,
/// opaque, resampled to [`LAYER_SIZE`], with mips (`None`: no diffuse
/// texture; the layer is then untextured white).
pub fn layer_texels(pack: &Pack, material: &Material) -> anyhow::Result<LayerLevels> {
    let id = material
        .diffuse_texture
        .ok_or_else(|| anyhow::anyhow!("material {} has no diffuse texture", material.id))?;
    let mut image = crate::texture::load_png(pack, id, 0)?;
    anyhow::ensure!(
        image.w > 0 && image.h > 0 && image.px.len() >= (image.w * image.h * 4) as usize,
        "texture {id}: empty or short"
    );
    let lut = crate::models::materials::gamma_lut();
    for p in image.px.chunks_exact_mut(4) {
        for c in &mut p[..3] {
            *c = lut[usize::from(*c)];
        }
        p[3] = 255;
    }
    image.px.truncate((image.w * image.h * 4) as usize);
    let px = resample_wrapped(image.w, image.h, &image.px, LAYER_SIZE);
    Ok(crate::models::materials::mip_chain(
        LAYER_SIZE, LAYER_SIZE, 4, true, px,
    ))
}
