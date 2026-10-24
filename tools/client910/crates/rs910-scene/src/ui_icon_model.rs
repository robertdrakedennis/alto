//! Inventory item icons: an item's model lit, projected and drawn into a
//! 36x32 canvas by the CPU rasteriser.
use crate::{
    actor_matrix::Matrix,
    cache::Pack,
    config::Obj,
    icon_raster::{
        IconRaster, IconTexture, ScreenPoint, TextureAlpha, TexturedPoint, Translucency,
    },
    model::RawModel,
    texture::{AlphaMode, MaterialStore},
};
use anyhow::{Context, Result};
use std::collections::BTreeMap;

pub struct Models {
    pub materials: MaterialStore,
    textured: BTreeMap<u32, bool>,
}
impl Models {
    pub fn new(pack: &Pack) -> Result<Self> {
        let textured = pack
            .read_group("materials", 0)?
            .into_iter()
            .map(|(id, b)| {
                let (at, mask) = if b[0] == 0 { (3, 1) } else { (1, 32) };
                (
                    id,
                    i32::from_be_bytes(b[at..at + 4].try_into().unwrap()) & mask != 0,
                )
            })
            .collect();
        Ok(Self {
            materials: MaterialStore::load(pack)?,
            textured,
        })
    }
    pub fn render(
        &self,
        pack: &Pack,
        obj: &Obj,
        outline: i32,
        enlarge: bool,
        light: [f32; 3],
    ) -> Result<Vec<i32>> {
        let id = *obj.models.first().context("inventory model missing")?;
        let files = pack.read_group("models", id)?;
        let bytes = files
            .get(&0)
            .context("inventory model file missing")?
            .as_slice();
        let mut raw = crate::model::decode(bytes)?;
        let tex = crate::model::decode_tex(bytes)?;
        if raw.version < 13 {
            for v in &mut raw.verts {
                for c in v {
                    *c = (*c as i32).wrapping_shl(2) as f32;
                }
            }
        }
        for c in &mut raw.colors {
            let mut value = (c[0] as u16) << 10 | (c[1] as u16) << 7 | c[2] as u16;
            for (i, (&s, &d)) in obj.recol_s.iter().zip(&obj.recol_d).enumerate() {
                if value == s {
                    value = if i < obj.inventory.recol_palette.len() {
                        0
                    } else {
                        d
                    };
                }
            }
            *c = [
                (value >> 10) as u8,
                (value >> 7 & 7) as u8,
                (value & 127) as u8,
            ];
        }
        for m in &mut raw.materials {
            for (&s, &d) in obj.retex_s.iter().zip(&obj.retex_d) {
                if *m == s as i32 {
                    *m = d as i32;
                }
            }
        }
        // UVs are built at construction; normals are computed lazily after scale.
        let uv = tex.corner_uvs_batch(&raw);
        for v in &mut raw.verts {
            for (c, &resize) in v.iter_mut().zip(&obj.inventory.resize) {
                *c = ((*c as i32).wrapping_mul(resize) >> 7) as f32;
            }
        }
        let normals = normals(&raw);
        let mesh = IconMesh {
            raw: &raw,
            uv: &uv,
            normals: &normals,
        };
        self.draw(pack, obj, mesh, outline, enlarge, light)
    }
    fn draw(
        &self,
        pack: &Pack,
        obj: &Obj,
        mesh: IconMesh<'_>,
        outline: i32,
        enlarge: bool,
        light: [f32; 3],
    ) -> Result<Vec<i32>> {
        let IconMesh { raw, uv, normals } = mesh;
        let cfg = &obj.inventory;
        let zoom = (if enlarge {
            (cfg.zoom as f64 * 1.5) as i32
        } else if outline == 2 {
            (cfg.zoom as f64 * 1.04) as i32
        } else {
            cfg.zoom
        }) << 2;
        let mut matrix = Matrix::axis(0., 0., 1., crate::trig::radians(-cfg.angles[2] << 3));
        matrix.rotate(0., 1., 0., crate::trig::radians(cfg.angles[1] << 3));
        let (sin, cos) = crate::minimap::trig1(cfg.angles[0] << 3);
        let min_y = raw
            .verts
            .iter()
            .take(
                raw.faces
                    .iter()
                    .flatten()
                    .max()
                    .map_or(0, |v| *v as usize + 1),
            )
            .map(|v| v[1] as i32)
            .min()
            .unwrap_or(0) as i16 as i32;
        matrix.0[9] += (cfg.offset[0] << 2) as f32;
        matrix.0[10] += ((sin.wrapping_mul(zoom) >> 14) - min_y / 2 + (cfg.offset[1] << 2)) as f32;
        matrix.0[11] += ((cfg.offset[1] << 2) + (cos.wrapping_mul(zoom) >> 14)) as f32;
        matrix.rotate(1., 0., 0., crate::trig::radians(cfg.angles[0] << 3));
        // The icon camera: a 512-unit view volume centred on the 36x32
        // canvas, near plane 50, effectively no far plane.
        let proj = crate::camera::perspective_pixels(crate::camera::PixelLens {
            centre: [16., 16.],
            focal: [512., 512.],
            near: 50.,
            far: 2.1474836e9,
            size: [36., 32.],
        });
        let m = crate::camera::multiply(&matrix.entries(), &proj);
        let points: Vec<[f32; 4]> = raw
            .verts
            .iter()
            .map(|&[x, y, z]| {
                let p: [f32; 4] =
                    std::array::from_fn(|i| m[8 + i] * z + m[i] * x + m[4 + i] * y + m[12 + i]);
                [
                    if p[2] >= -p[3] {
                        18. * p[0] / p[3] + 18.
                    } else {
                        -5000.
                    },
                    16. * p[1] / p[3] + 16.,
                    p[2] / p[3],
                    p[3],
                ]
            })
            .collect();
        if let Some(out) = crate::debug_flags::flags().icon_model_out.clone() {
            let b: Vec<u8> = points
                .iter()
                .flat_map(|p| p.iter().flat_map(|v| v.to_be_bytes()))
                .collect();
            std::fs::write(
                std::path::PathBuf::from(out).join(format!("rust-points-{}-{outline}.bin", obj.id)),
                b,
            )?;
        }
        let mut raster = IconRaster::new(36, 32);
        let mats: Vec<_> = raw
            .materials
            .iter()
            .map(|&id| {
                if id < 0 {
                    None
                } else {
                    self.materials.get(id as u32).filter(|m| !m.high_detail)
                }
            })
            .collect();
        for mat in mats.iter().flatten() {
            if self.textured.get(&mat.id) != Some(&true) || raster.has_texture(mat.id as i32) {
                continue;
            }
            let size = mat.size.context("inventory material size")? as i32;
            raster.add_texture(
                mat.id as i32,
                IconTexture {
                    texels: material_texels(pack, mat)?,
                    size,
                    average_colour: mat.average_colour as i32,
                    alpha: match mat.alpha {
                        AlphaMode::None => TextureAlpha::Opaque,
                        AlphaMode::AlphaTested => TextureAlpha::Cutout,
                        AlphaMode::Multiply => TextureAlpha::Blended,
                    },
                    alpha_threshold: mat.alpha_threshold as i32,
                    repeat: mat.repeat_s != 0 || mat.repeat_t != 0,
                },
            );
        }
        let sun_len = (5100.0f64).sqrt() as f32;
        let sun = [
            (-50. * 65535. / sun_len) as i32,
            (-10. * 65535. / sun_len) as i32,
            (-50. * 65535. / sun_len) as i32,
        ];
        let ambient = (light[0] * 65535.) as i32 >> 8;
        let contrast = cfg.contrast * 5 + 768;
        let strength = [
            (light[1] * 65535.) as i32 * 768 / contrast,
            (light[2] * 65535.) as i32 * 768 / contrast,
        ];
        let mut order: Vec<usize> = (0..raw.faces.len()).collect();
        let transparent = order.iter().any(|&i| {
            raw.alphas[i] != 0 || mats[i].is_some_and(|m| m.alpha == AlphaMode::Multiply)
        });
        if transparent {
            order.sort_by_key(|&i| {
                let mat = mats[i];
                let alpha =
                    raw.alphas[i] != 0 || mat.is_some_and(|m| m.alpha == AlphaMode::Multiply);
                let high = if alpha {
                    (raw.priorities[i] as i8 as i32) << 17 | 65536
                } else {
                    0
                };
                let high = high
                    + ((mat.map_or(0, |m| m.effect) as i32) << 8)
                    + mat.map_or(0, |m| m.effect_param) as i32;
                ((high as i64) << 32)
                    + ((mat.map_or(-1, |m| m.id as i32) & 65535) << 16)
                        .wrapping_add(i as i32 & 65535) as i64
            });
        }
        for i in order {
            let face = raw.faces[i].map(|v| v as usize);
            let [a, b, c] = face.map(|v| points[v]);
            if [a, b, c].iter().any(|p| p[0] == -5000.)
                || (a[0] - b[0]) * (c[1] - b[1]) - (c[0] - b[0]) * (a[1] - b[1]) <= 0.
            {
                continue;
            }
            let alpha = raw.alphas[i];
            let kind = if alpha == 254 {
                3
            } else if alpha == 255 {
                2
            } else {
                raw.face_types[i]
            };
            if kind == 2 || kind > 3 {
                continue;
            }
            let base = ((raw.colors[i][0] as i32) << 10)
                | ((raw.colors[i][1] as i32) << 7)
                | raw.colors[i][2] as i32;
            let hsv = crate::colour::renormalise_saturation(
                (base & !127) | (((base & 127) * (cfg.ambient + 64)) >> 7),
            );
            let mut colours = [0; 3];
            for corner in 0..3 {
                let normal = if kind == 1 {
                    normals.face[i]
                } else {
                    normals.vertex[face[corner]]
                };
                let dot = (normal[2]
                    .wrapping_mul(sun[2])
                    .wrapping_add(normal[0].wrapping_mul(sun[0]))
                    .wrapping_add(normal[1].wrapping_mul(sun[1]))
                    / normal[3].max(1))
                    >> 16;
                let factor = if dot > 256 { strength[0] } else { strength[1] };
                colours[corner] = if let Some(mat) = mats[i] {
                    let bright = ((dot.wrapping_mul(factor) >> 18) + (ambient >> 2)).clamp(2, 126);
                    bright << 24 | material_colour(base, bright, mat)
                } else {
                    let bright = (dot.wrapping_mul(factor) >> 17) + (ambient >> 1);
                    bright << 17 | crate::colour::scale_lightness(hsv, bright)
                };
            }
            if kind == 1 {
                colours = [colours[0]; 3];
            } else if kind == 3 {
                colours = [128; 3];
            }
            let corners = [a, b, c].map(|p| ScreenPoint {
                x: p[0],
                y: p[1],
                depth: p[2],
            });
            if let Some(mat) = mats[i].filter(|m| self.textured.get(&m.id) == Some(&true)) {
                let uv = uv[i].context("inventory texture UV mapping")?;
                let textured = std::array::from_fn(|corner| TexturedPoint {
                    at: corners[corner],
                    w: [a, b, c][corner][3],
                    u: uv[corner][0],
                    v: uv[corner][1],
                    light: ((255 - alpha as i32) << 24) | (colours[corner] & 0xffffff),
                });
                raster.fill_textured(textured, mat.id as i32);
            } else {
                let translucency = Translucency::from_level(alpha as i32);
                if kind == 1 || kind == 3 {
                    let colour = raster.palette_colour(colours[0]);
                    raster.fill_flat(corners, colour, translucency);
                } else {
                    raster.fill_palette_shaded(corners, colours.map(|c| c & 65535), translucency);
                }
            }
        }
        Ok(raster.into_pixels())
    }
}
/// The scaled model, its corner UVs and normals for one icon draw.
struct IconMesh<'a> {
    raw: &'a RawModel,
    uv: &'a [Option<[[f32; 2]; 3]>],
    normals: &'a Normals,
}
struct Normals {
    vertex: Vec<[i32; 4]>,
    face: Vec<[i32; 4]>,
}
fn normals(raw: &RawModel) -> Normals {
    let mut out = Normals {
        vertex: vec![[0; 4]; raw.verts.len()],
        face: vec![[0; 4]; raw.faces.len()],
    };
    for (i, f) in raw.faces.iter().enumerate() {
        let [a, b, c] = f.map(|i| raw.verts[i as usize].map(|v| v as i32));
        let u: [i32; 3] = std::array::from_fn(|i| b[i].wrapping_sub(a[i]));
        let v: [i32; 3] = std::array::from_fn(|i| c[i].wrapping_sub(a[i]));
        let mut n: [i32; 3] = std::array::from_fn(|i| {
            u[(i + 1) % 3]
                .wrapping_mul(v[(i + 2) % 3])
                .wrapping_sub(u[(i + 2) % 3].wrapping_mul(v[(i + 1) % 3]))
        });
        while n.iter().any(|&v| !(-8192..=8192).contains(&v)) {
            for v in &mut n {
                *v >>= 1;
            }
        }
        let len = ((n[2] * n[2] + n[0] * n[0] + n[1] * n[1]) as f64).sqrt() as i32;
        for v in &mut n {
            *v = *v * 256 / len.max(1);
        }
        out.face[i] = [n[0], n[1], n[2], 1];
        if raw.face_types[i] == 0 {
            for &v in f {
                for (dst, &nk) in out.vertex[v as usize].iter_mut().zip(&n) {
                    *dst += nk;
                }
                out.vertex[v as usize][3] += 1;
            }
        }
    }
    out
}
/// A material's diffuse map as the icon and software renderers sample it:
/// gamma-adjusted (brightness 0.7), then softened with a 3x3 wrap-around blur
/// unless the material is alpha-tested. `None` for a material without a
/// diffuse map.
pub fn material_texels(pack: &Pack, mat: &crate::texture::Material) -> Result<Option<Vec<i32>>> {
    let size = mat.size.context("inventory material size")? as i32;
    let Some(id) = mat.diffuse_texture else {
        return Ok(None);
    };
    let image = match crate::texture::load_texture(pack, id)? {
        crate::texture::Texture::Single(i) => i,
        _ => anyhow::bail!("inventory diffuse cubemap"),
    };
    anyhow::ensure!(
        image.w == size as u32 && image.h == size as u32,
        "inventory texture size mismatch"
    );
    let gamma: Vec<i32> = (0..256)
        .map(|v| ((v as f64 / 255.).powf(0.7f32 as f64) * 255.) as i32)
        .collect();
    let mut p: Vec<i32> = image
        .px
        .chunks_exact(4)
        .map(|p| {
            let rgb = gamma[p[0] as usize] << 16 | gamma[p[1] as usize] << 8 | gamma[p[2] as usize];
            let a = if mat.alpha == AlphaMode::None {
                if rgb == 0 {
                    0
                } else {
                    255
                }
            } else {
                p[3] as i32
            };
            a << 24 | rgb
        })
        .collect();
    if mat.alpha != AlphaMode::AlphaTested {
        p = blur(&p, size as usize);
    }
    Ok(Some(p))
}
pub fn material_colour(hsl: i32, bright: i32, m: &crate::texture::Material) -> i32 {
    let mut rgb =
        crate::colour::hsl_tables().rgb[crate::colour::scale_lightness(hsl, bright) as usize];
    let grey = m.grey_blend as i32;
    if grey != 0 {
        let target = bright * 131586;
        let inv = 256 - grey;
        rgb = (((rgb & 0xff00ff)
            .wrapping_mul(inv)
            .wrapping_add((target & 0xff00ff).wrapping_mul(grey))
            & 0xff00ff00u32 as i32)
            + (((rgb & 0xff00) * inv + (target & 0xff00) * grey) & 0xff0000))
            >> 8;
    }
    if m.brightness_boost != 0 {
        let b = 256 + m.brightness_boost as i32;
        rgb = ((((rgb >> 16 & 255) * b).min(65535) & 0xff00) << 8)
            | (((rgb >> 8 & 255) * b).min(65535) & 0xff00)
            | ((rgb & 255) * b).min(65535) >> 8;
    }
    rgb
}
fn blur(p: &[i32], size: usize) -> Vec<i32> {
    (0..p.len())
        .map(|i| {
            let (x, y) = (i % size, i / size);
            let mut sum = [0; 4];
            for dy in [size - 1, 0, 1] {
                for dx in [size - 1, 0, 1] {
                    let v = p[((y + dy) % size) * size + (x + dx) % size];
                    for (c, s) in sum.iter_mut().enumerate() {
                        *s += v >> (c * 8) & 255;
                    }
                }
            }
            sum.iter()
                .enumerate()
                .fold(0, |v, (c, s)| v | (s / 9) << (c * 8))
        })
        .collect()
}

#[cfg(test)]
mod tests;
