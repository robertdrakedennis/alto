//! The sprites of the sky's decorations (sun, moon, clouds): what a decor draws
//! as one square picture, baked on the CPU so every renderer draws the same
//! pixels ([`crate::sky_frame::SkyCache`] bakes and keeps them).
//!
//! - **Texture** decors are a texture drawn as is (its first `size * size`
//!   texels, the original's row stride being the sprite size).
//! - **Lit sphere** decors are a hemisphere facing the viewer, lit by the
//!   sky's sun, textured with the decor's material, with a halo tinted by the
//!   decor's colour and a soft round edge.
//! - **Model** decors are the decor's model turned by its rotation, drawn
//!   orthographically into the sprite and lit by the sun.
//!
//! Where the sun lies relative to a decor is the decor's own frame: the light
//! direction is turned by the decor's pitch and yaw so the lit side faces the
//! sun as the viewer sees it.

use crate::skybox::SkyboxDecor;
use anyhow::{Context, Result};

/// Ambient share of a lit decor's shading.
const AMBIENT: f32 = 0.55;

/// The decor kinds of the config type.
const KIND_TEXTURE: i32 = 0;
const KIND_SPHERE: i32 = 1;
const KIND_MODEL: i32 = 2;

/// Bake `decor` at `size` pixels square, lit by `sun` (the decor the sky
/// lights the others with; `None`: a light straight along the view).
pub fn bake(
    decor: &SkyboxDecor,
    size: i32,
    sun: Option<&SkyboxDecor>,
    assets: &crate::sky_texture::SkyAssets<'_>,
) -> Result<crate::sky_texture::SkyTexture> {
    anyhow::ensure!((1..=512).contains(&size), "decor sprite size {size}");
    let argb = match decor.kind {
        KIND_TEXTURE => texture_sprite(decor, size, assets)?,
        KIND_SPHERE => sphere_sprite(decor, size, sun, assets)?,
        KIND_MODEL => model_sprite(decor, size, sun, assets)?,
        other => anyhow::bail!("sky decor kind {other}"),
    };
    Ok(crate::sky_texture::SkyTexture {
        first: argb[0],
        last: argb[argb.len() - 1],
        size: [size, size],
        argb,
    })
}

/// The gamma 0.7 table of the sky's textures.
fn gamma_table() -> [u8; 256] {
    let mut lut = [0_u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        *slot = (((i as f64 / 255.0).powf(0.7) * 255.0) as i32).min(255) as u8;
    }
    lut
}

fn pack_argb(r: u8, g: u8, b: u8, a: u8) -> i32 {
    ((u32::from(a) << 24) | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)) as i32
}

/// Texture decors: the texture id's first `size * size` texels at gamma 0.7.
fn texture_sprite(
    decor: &SkyboxDecor,
    size: i32,
    assets: &crate::sky_texture::SkyAssets<'_>,
) -> Result<Vec<i32>> {
    let id = u32::try_from(decor.texture).context("decor texture id")?;
    let image = match crate::texture::load_texture(assets.pack, id)? {
        crate::texture::Texture::Single(image) => image,
        crate::texture::Texture::Cube(faces) => {
            faces.into_iter().next().context("empty cube texture")?
        }
    };
    let count = (size * size) as usize;
    let lut = gamma_table();
    anyhow::ensure!(
        image.px.len() / 4 >= count,
        "decor texture {id} has {} texels, the sprite needs {count}",
        image.px.len() / 4
    );
    Ok(image
        .px
        .chunks_exact(4)
        .take(count)
        .map(|p| {
            pack_argb(
                lut[p[0] as usize],
                lut[p[1] as usize],
                lut[p[2] as usize],
                p[3],
            )
        })
        .collect())
}

/// The light direction of a decor's frame, in the sprite's view axes: the
/// sun's position relative to the decor (a fixed sun is a direction), turned
/// by the decor's pitch then yaw; straight along the view without a sun.
fn light_direction(decor: &SkyboxDecor, sun: Option<&SkyboxDecor>, from_sun: bool) -> [f32; 3] {
    let (mut x, mut y, mut z) = (0_i32, 0_i32, 256_i32);
    if let Some(sun) = sun {
        let relative = |s: i32, d: i32| if sun.fixed { -s } else { d - s };
        // Model decors take the vector from the sun, spheres towards it.
        let (dx, dy, dz) = if from_sun {
            (
                relative(sun.position[0], decor.position[0]),
                relative(sun.position[1], decor.position[1]),
                relative(sun.position[2], decor.position[2]),
            )
        } else {
            let towards = |s: i32, d: i32| if sun.fixed { -s } else { s - d };
            (
                towards(sun.position[0], decor.position[0]),
                towards(sun.position[1], decor.position[1]),
                towards(sun.position[2], decor.position[2]),
            )
        };
        (x, y, z) = (dx, dy, dz);
    }
    let sign = if from_sun { -1 } else { 1 };
    if decor.pitch != 0 {
        let angle = sign * decor.pitch;
        let (sin, cos) = (rs910_core::trig::sin(angle), rs910_core::trig::cos(angle));
        let rotated = (y.wrapping_mul(cos).wrapping_sub(z.wrapping_mul(sin))) >> 14;
        z = (y.wrapping_mul(sin).wrapping_add(z.wrapping_mul(cos))) >> 14;
        y = rotated;
    }
    if decor.yaw != 0 {
        let angle = sign * decor.yaw;
        let (sin, cos) = (rs910_core::trig::sin(angle), rs910_core::trig::cos(angle));
        let rotated = (x.wrapping_mul(cos).wrapping_add(z.wrapping_mul(sin))) >> 14;
        z = (z.wrapping_mul(cos).wrapping_sub(x.wrapping_mul(sin))) >> 14;
        x = rotated;
    }
    let v = [x as f32, y as f32, z as f32];
    let length = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if length == 0.0 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / length, v[1] / length, v[2] / length]
    }
}

/// Lambert term of a surface normal under a light travelling along `light`.
fn lambert(normal: [f32; 3], light: [f32; 3]) -> f32 {
    (-(normal[0] * light[0] + normal[1] * light[1] + normal[2] * light[2])).max(0.0)
}

/// The rgb (0..=1) of a decor colour.
fn colour_rgb(colour: i32) -> [f32; 3] {
    [
        ((colour >> 16) & 0xFF) as f32 / 255.0,
        ((colour >> 8) & 0xFF) as f32 / 255.0,
        (colour & 0xFF) as f32 / 255.0,
    ]
}

/// The soft-edge falloff of the 128-texel round mask at normalised offset
/// `(dx, dy)` from the centre (`-1..=1`): 255 in the middle, 0 at the rim.
fn round_mask(dx: f32, dy: f32) -> u8 {
    // 64 texels from the centre is the rim: (d * 64)^2 * 256 / 4096.
    let d2 = (dx * dx + dy * dy) * 64.0 * 64.0;
    let value = 256.0 - d2 * 256.0 / 4096.0;
    (value * 2.0).clamp(0.0, 255.0) as u8
}

/// The halo's alpha at the same offset: 127 at the rim, none in the middle.
fn halo_alpha(dx: f32, dy: f32) -> u8 {
    127 - round_mask(dx, dy) / 2
}

/// Lit sphere decors (see the module docs).
fn sphere_sprite(
    decor: &SkyboxDecor,
    size: i32,
    sun: Option<&SkyboxDecor>,
    assets: &crate::sky_texture::SkyAssets<'_>,
) -> Result<Vec<i32>> {
    let texture = crate::sky_texture::load_texture(assets, decor.texture).ok();
    let light = light_direction(decor, sun, false);
    let tint = colour_rgb(decor.colour);
    let half = size as f32 / 2.0;
    // The sphere's radius in the sprite; a coloured decor leaves room for
    // its halo.
    let radius = if decor.colour & 0xFF_FFFF == 0 && decor.colour >> 24 == 0 {
        half
    } else {
        half * 13.0 / 16.0
    };
    // The halo is drawn over a square of 13/16 of the sprite.
    let halo_size = size as f32 * 13.0 / 16.0;
    let halo_from = (size as f32 - halo_size) / 2.0;
    let mut out = vec![0_i32; (size * size) as usize];
    for py in 0..size {
        for px in 0..size {
            let fx = px as f32 + 0.5;
            let fy = py as f32 + 0.5;
            let (nx, ny) = ((fx - half) / radius, (fy - half) / radius);
            let r2 = nx * nx + ny * ny;
            let (mut rgb, mut alpha) = ([0.0_f32; 3], 0.0_f32);
            if r2 <= 1.0 {
                let normal = [nx, ny, -(1.0 - r2).sqrt()];
                let lit = AMBIENT + (1.0 - AMBIENT) * lambert(normal, light);
                let base = texture.as_ref().map_or([1.0; 3], |t| {
                    // Longitude across the visible half, latitude pole to pole.
                    let u = 0.5 + normal[0].atan2(-normal[2]) / std::f32::consts::PI;
                    let v = 0.5 + (-normal[1]).asin() / std::f32::consts::PI;
                    let [tw, th] = t.size;
                    let x = ((u.clamp(0.0, 0.9999) * tw as f32) as i32).clamp(0, tw - 1);
                    let y = ((v.clamp(0.0, 0.9999) * th as f32) as i32).clamp(0, th - 1);
                    let texel = t.argb[(y * tw + x) as usize] as u32;
                    [
                        ((texel >> 16) & 0xFF) as f32 / 255.0,
                        ((texel >> 8) & 0xFF) as f32 / 255.0,
                        (texel & 0xFF) as f32 / 255.0,
                    ]
                });
                rgb = [base[0] * lit, base[1] * lit, base[2] * lit];
                alpha = 1.0;
            }
            // The halo over the 13/16 square: the decor colour, fading in
            // towards the rim.
            let (hx, hy) = (
                (fx - halo_from) / halo_size * 2.0 - 1.0,
                (fy - halo_from) / halo_size * 2.0 - 1.0,
            );
            if hx.abs() <= 1.0 && hy.abs() <= 1.0 {
                let a = f32::from(halo_alpha(hx, hy)) / 255.0;
                for c in 0..3 {
                    rgb[c] = rgb[c] * (1.0 - a) + tint[c] * a;
                }
                alpha = alpha + a * (1.0 - alpha);
            }
            // The round soft edge over the whole sprite.
            let (mx, my) = ((fx - half) / half, (fy - half) / half);
            alpha *= f32::from(round_mask(mx, my)) / 255.0;
            let to_byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            out[(py * size + px) as usize] = pack_argb(
                to_byte(rgb[0]),
                to_byte(rgb[1]),
                to_byte(rgb[2]),
                to_byte(alpha),
            );
        }
    }
    Ok(out)
}

/// Model decors (see the module docs): the model built with the sky's own
/// lighting flags, orthographic, near side in front.
fn model_sprite(
    decor: &SkyboxDecor,
    size: i32,
    sun: Option<&SkyboxDecor>,
    assets: &crate::sky_texture::SkyAssets<'_>,
) -> Result<Vec<i32>> {
    let id = u32::try_from(decor.texture).context("decor model id")?;
    let mut raw = crate::modelunlit::ModelUnlit::load(assets.pack, id)?;
    raw.rotate(
        decor.rotation[0] & 0x3FFF,
        decor.rotation[1] & 0x3FFF,
        decor.rotation[2] & 0x3FFF,
    );
    let mut model = crate::gpumodel::GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: assets.materials,
            billboards: assets.billboards,
            emitters: assets.emitters,
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: 2048,
            ambient: 0,
            contrast: 64,
            detail: 768,
        },
    )?;
    let extent = (model.max_x() - model.min_x()).max(model.max_y() - model.min_y());
    let extent = extent.max(1) as f32;
    let depth_offset = (50 - model.min_z()) as f32;
    let positions = model.position_stream();
    let normals = model.normal_stream();
    let albedo = model.albedo_stream(assets.materials)?;
    let indices = model.index_stream();
    let light = light_direction(decor, sun, true);
    let sun_rgb = colour_rgb(decor.colour);
    let scale = size as f32 / extent;
    let centre = size as f32 / 2.0;
    // Per vertex: screen position, depth and lit colour.
    let vertices: Vec<([f32; 3], [f32; 3])> = positions
        .iter()
        .zip(&normals)
        .zip(&albedo)
        .map(|((p, n), &colour)| {
            let lit = lambert(*n, light);
            let base = colour_rgb(colour);
            let shade = |c: usize| (AMBIENT + sun_rgb[c] * lit * (1.0 - AMBIENT)).min(1.0);
            (
                [
                    centre + p[0] * scale,
                    centre + p[1] * scale,
                    p[2] + depth_offset,
                ],
                [base[0] * shade(0), base[1] * shade(1), base[2] * shade(2)],
            )
        })
        .collect();
    let mut depth = vec![f32::INFINITY; (size * size) as usize];
    let mut out = vec![0_i32; (size * size) as usize];
    for triangle in indices.chunks_exact(3) {
        let [a, b, c] = [triangle[0], triangle[1], triangle[2]].map(|i| vertices[usize::from(i)]);
        rasterise(size, [a, b, c], &mut depth, &mut out);
    }
    Ok(out)
}

/// Fill one Gouraud triangle into `out` with a depth test.
fn rasterise(size: i32, corners: [([f32; 3], [f32; 3]); 3], depth: &mut [f32], out: &mut [i32]) {
    let [(p0, c0), (p1, c1), (p2, c2)] = corners;
    let area = (p1[0] - p0[0]) * (p2[1] - p0[1]) - (p2[0] - p0[0]) * (p1[1] - p0[1]);
    if area.abs() < 1e-6 {
        return;
    }
    let min_x = p0[0].min(p1[0]).min(p2[0]).floor().max(0.0) as i32;
    let max_x = (p0[0].max(p1[0]).max(p2[0]).ceil() as i32).min(size - 1);
    let min_y = p0[1].min(p1[1]).min(p2[1]).floor().max(0.0) as i32;
    let max_y = (p0[1].max(p1[1]).max(p2[1]).ceil() as i32).min(size - 1);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = ((p1[0] - fx) * (p2[1] - fy) - (p2[0] - fx) * (p1[1] - fy)) / area;
            let w1 = ((p2[0] - fx) * (p0[1] - fy) - (p0[0] - fx) * (p2[1] - fy)) / area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = w0 * p0[2] + w1 * p1[2] + w2 * p2[2];
            let slot = (y * size + x) as usize;
            if z >= depth[slot] {
                continue;
            }
            depth[slot] = z;
            let channel = |i: usize| {
                ((w0 * c0[i] + w1 * c1[i] + w2 * c2[i]).clamp(0.0, 1.0) * 255.0 + 0.5) as u8
            };
            out[slot] = pack_argb(channel(0), channel(1), channel(2), 255);
        }
    }
}

#[cfg(test)]
mod tests;
