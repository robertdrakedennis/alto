//! Texture pixels built from the first font sprite. This is the
//! CPU resource consumed by the forthcoming shared GPU interface renderer.
use crate::{font_metrics::Metrics, sprite_sheet::SpriteSheet};

#[derive(Clone, Debug)]
pub struct Atlas {
    pub width: u16,
    pub height: u16,
    pub scale: u8,
    pub argb: Vec<u32>,
}

impl Atlas {
    pub fn new(m: &Metrics, sheet: &SpriteSheet, monochrome: bool) -> anyhow::Result<Self> {
        let mut pixels = vec![0u32; m.atlas_width as usize * m.atlas_height as usize];
        match sheet {
            SpriteSheet::Paletted {
                palette, sprites, ..
            } => {
                let s = sprites
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("font sprite missing"))?;
                anyhow::ensure!(s.colour.len() <= pixels.len(), "font sprite exceeds atlas");
                // An all-255 alpha plane is discarded.
                let alpha = s.alpha.as_ref().filter(|a| a.iter().any(|v| *v != 255));
                for (i, &index) in s.colour.iter().enumerate() {
                    pixels[i] = if monochrome {
                        let a = alpha.map_or(if index == 0 { 0 } else { 255 }, |p| p[i]);
                        (a as u32) << 24 | 0xffffff
                    } else if let Some(a) = alpha {
                        (a[i] as u32) << 24 | palette[index as usize]
                    } else if index == 0 {
                        0
                    } else {
                        0xff000000 | palette[index as usize]
                    };
                }
            }
            SpriteSheet::Full { sprites, .. } => {
                let s = sprites
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("font sprite missing"))?;
                let raw: Vec<u32> = s
                    .rgb
                    .chunks_exact(3)
                    .enumerate()
                    .map(|(i, rgb)| {
                        let rgb = (rgb[0] as u32) << 16 | (rgb[1] as u32) << 8 | rgb[2] as u32;
                        let p = if rgb == 0xff00ff { 0 } else { 0xff000000 | rgb };
                        // Alpha overwrites the magenta transparency, retaining RGB=0.
                        s.alpha
                            .as_ref()
                            .map_or(p, |a| (p & 0xffffff) | (a[i] as u32) << 24)
                    })
                    .collect();
                let translucent = raw.iter().any(|p| p >> 24 != 255);
                if monochrome {
                    anyhow::ensure!(raw.len() <= pixels.len(), "font sprite exceeds atlas");
                    for (i, p) in raw.into_iter().enumerate() {
                        // Reference quirk: the translucent branch reads alpha
                        // from the new zero-filled output, not the source pixels.
                        let a = if translucent {
                            0
                        } else {
                            ((p & 255) + 3 * ((p >> 16) & 255) + 4 * ((p >> 8) & 255)) >> 3
                        };
                        pixels[i] = a << 24 | 0xffffff;
                    }
                } else {
                    anyhow::ensure!(raw.len() >= pixels.len(), "font sprite shorter than atlas");
                    for (out, &p) in pixels.iter_mut().zip(&raw) {
                        *out = if translucent {
                            p
                        } else {
                            let rgb = p & 0xffffff;
                            rgb | if rgb == 0 { 0 } else { 0xff000000 }
                        };
                    }
                }
            }
        }
        Ok(Self {
            width: m.atlas_width,
            height: m.atlas_height,
            scale: m.scale,
            argb: pixels,
        })
    }
}
