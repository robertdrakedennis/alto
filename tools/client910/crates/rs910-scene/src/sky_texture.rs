//! What the sky's CPU loads read and produce: the cache stores the loads
//! read ([`SkyAssets`]), a texture as ARGB texels ([`SkyTexture`]) and the
//! material texture load. Shared by the frame's resolution
//! ([`crate::sky_frame`]) and the decor sprite bake ([`crate::sky_decor`]).
/// The stores the sky model and material texture loads read (models, materials, textures).
pub struct SkyAssets<'a> {
    pub pack: &'a crate::cache::Pack,
    pub materials: &'a crate::texture::MaterialStore,
    pub billboards: &'a crate::billboard::BillboardStore,
    pub emitters: &'a crate::particle::EmitterStore,
}

/// A material layer's texture at gamma 0.7 (a multiply material keeps its alpha) with its
/// first/last pixels.
pub struct SkyTexture {
    pub argb: Vec<i32>,
    pub size: [i32; 2],
    pub first: i32,
    pub last: i32,
}

/// Load a material's diffuse map at gamma 0.7: black texels get zero alpha and the rest
/// become opaque, except that a multiply material keeps its alpha.
///
/// The requested size is ignored by the texture list: the sprite is the texture at its
/// native size, as here.
pub(crate) fn load_texture(assets: &SkyAssets<'_>, material_id: i32) -> anyhow::Result<SkyTexture> {
    let material = assets
        .materials
        .get(u32::try_from(material_id)?)
        .ok_or_else(|| anyhow::anyhow!("material {material_id} missing"))?;
    let texture_id = material
        .diffuse_texture
        .ok_or_else(|| anyhow::anyhow!("material {material_id} has no diffuse map"))?;
    let image = match crate::texture::load_texture(assets.pack, texture_id)? {
        crate::texture::Texture::Single(image) => image,
        crate::texture::Texture::Cube(faces) => faces
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("texture {texture_id}: empty cube"))?,
    };
    let multiply = material.alpha == crate::texture::AlphaMode::Multiply;
    let mut lut = [0_u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let v = ((i as f64 / 255.0).powf(0.7) * 255.0) as i32;
        *slot = v.min(255) as u8;
    }
    let mut px = image.px.clone();
    for p in px.chunks_exact_mut(4) {
        p[0] = lut[p[0] as usize];
        p[1] = lut[p[1] as usize];
        p[2] = lut[p[2] as usize];
        if !multiply {
            p[3] = if p[0] == 0 && p[1] == 0 && p[2] == 0 {
                0
            } else {
                255
            };
        }
    }
    let argb = |p: &[u8]| {
        (u32::from(p[3]) << 24) | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])
    };
    let first = argb(&px[..4]);
    let last = argb(&px[px.len() - 4..]);
    Ok(SkyTexture {
        argb: px.chunks_exact(4).map(|p| argb(p) as i32).collect(),
        size: [image.w as i32, image.h as i32],
        first: first as i32,
        last: last as i32,
    })
}

impl SkyTexture {
    /// The texels as RGBA bytes (the `px` the texture was built from; `argb`
    /// packs each texel's four bytes, so this is exact).
    #[must_use]
    pub fn rgba(&self) -> Vec<u8> {
        self.argb
            .iter()
            .flat_map(|&c| {
                let c = c as u32;
                [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
            })
            .collect()
    }
}
