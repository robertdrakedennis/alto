//! World-map scene icon loading, tinting and transforms.

use crate::{cache::Pack, minimap::MsiStore, ui_sprites::Sprite};

use anyhow::Result;

use std::{collections::HashMap, rc::Rc};

/// The map scene icons as the world map uses them: the sprite size once its
/// padding is dropped, and the tinted pixel array of the *most recently
/// built* rotation of that type (the one array per type is overwritten on
/// every sprite-cache miss).
#[derive(Default)]
pub struct MapScenes {
    types: MsiStore,
    sprites: HashMap<(i32, i32, bool), Option<[i32; 2]>>,
    pixels: HashMap<i32, Rc<Vec<i32>>>,
}

impl MapScenes {
    pub fn new(types: MsiStore) -> Self {
        Self {
            types,
            ..Self::default()
        }
    }
    /// The tint loop, in wrapping `i32` arithmetic.
    pub fn tint(pixels: &mut [i32], tint: i32) {
        let r = tint >> 16 & 0xFF;
        let g = tint >> 8 & 0xFF;
        let b = tint & 0xFF;
        for p in pixels {
            let px = *p;
            let alpha = px >> 24 & 0xFF;
            let inverse = 256 - alpha;
            if alpha == 0 {
                continue;
            }
            let v14 = (px & 0xFF0000).wrapping_mul(r).wrapping_mul(-16_777_216);
            let v15 = (px & 0xFF00).wrapping_mul(g).wrapping_mul(16_711_680);
            let v16 = (px & 0xFF).wrapping_mul(b).wrapping_mul(65_280);
            let v17 = ((v14 | v15 | v16) as u32 >> 8) as i32;
            *p = if alpha == 255 {
                v17
            } else {
                let a = (v17 & 0xFF00FF)
                    .wrapping_mul(inverse)
                    .wrapping_add((px & 0xFF00FF).wrapping_mul(alpha))
                    & 0xFF00FF00u32 as i32;
                let c = (v17 & 0xFF00)
                    .wrapping_mul(inverse)
                    .wrapping_add((px & 0xFF00).wrapping_mul(alpha))
                    & 0xFF0000;
                alpha << 24 | a.wrapping_add(c) >> 8
            };
        }
    }
    /// The sprite size and pixel array the icon painter reads for `msi` drawn
    /// with `rotation`/`mirror`; `None` when the type has no sprite.
    pub fn icon(
        &mut self,
        pack: &Pack,
        msi: i32,
        rotation: i32,
        mirror: bool,
    ) -> Option<([i32; 2], Rc<Vec<i32>>)> {
        let t = self.types.get(msi);
        if t.sprite == -1 {
            return None;
        }
        let key = (t.sprite, rotation, mirror);
        let size = match self.sprites.get(&key) {
            Some(size) => *size,
            None => {
                let built = (|| -> Result<Option<([i32; 2], Vec<i32>)>> {
                    let Ok(mut files) = pack.read_group("sprites", t.sprite as u32) else {
                        return Ok(None);
                    };
                    let Some(bytes) = files.remove(&0) else {
                        return Ok(None);
                    };
                    let mut all = crate::sprite_data::Data::decode(&bytes)?;
                    if all.is_empty() {
                        return Ok(None);
                    }
                    let mut data = all.remove(0);
                    // Drop the padding.
                    data.padding = [0; 4];
                    if mirror {
                        data.flip(true);
                    }
                    for _ in 0..rotation {
                        data.rotate();
                    }
                    let mut pixels = data.argb(false)?;
                    if t.tint != 0 {
                        Self::tint(&mut pixels, t.tint);
                    }
                    let sprite = Sprite::new(&data)?;
                    Ok(Some((sprite.full_size(), pixels)))
                })()
                .unwrap_or_else(|error| {
                    log::warn!("[client910] world-map msi {msi} sprite unavailable: {error:#}");
                    None
                });
                let size = built.as_ref().map(|(size, _)| *size);
                if let Some((_, pixels)) = built {
                    self.pixels.insert(msi, Rc::new(pixels));
                }
                self.sprites.insert(key, size);
                size
            }
        }?;
        Some((size, self.pixels.get(&msi)?.clone()))
    }
    /// Forgets every built icon, as the load steps' type-cache resize does. The
    /// pixel array of a type is then rebuilt from the next orientation asked
    /// for.
    pub fn reset(&mut self) {
        self.sprites.clear();
        self.pixels.clear();
    }
    pub fn scale_to_loc(&self, msi: i32) -> bool {
        self.types.get(msi).scale_to_loc
    }
}
