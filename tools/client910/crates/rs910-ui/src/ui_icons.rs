//! buildIcon / renderInventoryIcon, 36 by 32 pixels.
use crate::{
    cache::Pack,
    config::ObjStore,
    ui_component_fields::Fields,
    ui_fonts::{Font, Fonts},
    ui_sprites::Sprite,
};
use anyhow::{Context, Result};
use std::{
    collections::{BTreeMap, VecDeque},
    rc::Rc,
};
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Key {
    id: i32,
    count: i32,
    outline: i32,
    shadow: i32,
    mode: i32,
    enlarge: bool,
    wear: bool,
}
pub struct Icons {
    pack: Pack,
    pub objs: Rc<ObjStore>,
    models: crate::ui_icon_model::Models,
    font: Rc<Font>,
    colours: [i32; 3],
    cache: BTreeMap<Key, Rc<Sprite>>,
    random: crate::font_layout::Random,
    palette: Option<crate::avatar::AvatarPalette>,
    defaults: crate::avatar::GraphicsDefaults,
    order: VecDeque<Key>,
}
impl Icons {
    pub fn new(pack: Pack, objs: Rc<ObjStore>, fonts: &Fonts) -> Result<Self> {
        let id = *fonts
            .ids
            .as_ref()
            .and_then(|v| v.first())
            .context("p11 font id")?;
        let font = fonts
            .get_font(id, true, true)?
            .context("p11 icon count font")?;
        let bytes =
            crate::js5_fetch::fetch_file(&pack, "defaults", 3)?.context("graphics defaults")?;
        let graphics = crate::protocol910::defaults::Graphics::decode(&bytes)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?
            .value
            .scalars;
        let models = crate::ui_icon_model::Models::new(&pack)?;
        let mut random = crate::font_layout::Random::default();
        random.set_seed(crate::logic_clock::monotonic_millis());
        Ok(Self {
            pack: pack.clone(),
            objs,
            models,
            font,
            colours: [
                graphics.inv_hundred_color,
                graphics.inv_thousand_color,
                graphics.inv_million_color,
            ],
            cache: BTreeMap::new(),
            random,
            palette: None,
            defaults: crate::avatar::GraphicsDefaults::load(&pack)?,
            order: VecDeque::new(),
        })
    }
    /// Clears the icon cache (the icon half of a cache reset).
    pub fn reset(&mut self) {
        self.cache.clear();
        self.order.clear();
    }
    pub fn set_palette(&mut self, palette: Option<crate::avatar::AvatarPalette>) {
        if self.palette != palette {
            self.palette = palette;
            self.cache.retain(|k, _| !k.wear);
            self.order.retain(|k| !k.wear);
        }
    }
    pub fn sprite(&mut self, f: &Fields) -> Result<Rc<Sprite>> {
        self.build(
            Key {
                id: f.invobject,
                count: f.invcount,
                outline: f.outline,
                shadow: f.graphicshadow | 0xff000000u32 as i32,
                mode: f.obj_count_display,
                enlarge: false,
                wear: f.usePlayerModel,
            },
            0,
        )
    }
    fn build(&mut self, key: Key, depth: usize) -> Result<Rc<Sprite>> {
        anyhow::ensure!(depth < 32, "cyclic inventory icon overlay");
        if let Some(sprite) = self.cache.get(&key) {
            self.order.retain(|k| *k != key);
            self.order.push_back(key);
            return Ok(sprite.clone());
        }
        let mut obj = self
            .objs
            .get(key.id as u32)
            .context("inventory obj missing")?;
        if key.count > 1 {
            if let Some(counts) = &obj.inventory.countobj {
                let mut selected = None;
                for &(id, count) in counts {
                    if count != 0 && key.count >= count {
                        selected = Some(id);
                    }
                }
                if let Some(id) = selected {
                    obj = self.objs.get(id as u32).context("stack variant missing")?;
                }
            }
        }
        let mut obj = obj.clone();
        let overlay = obj
            .inventory
            .derived
            .iter()
            .enumerate()
            .find(|(_, p)| p[1] != -1)
            .map(|(kind, &[id, _])| (kind, id));
        let over = if let Some((kind, id)) = overlay {
            let cert = kind == 0 || kind == 3;
            Some(self.build(
                Key {
                    id,
                    count: if cert { 10 } else { key.count },
                    outline: if cert { 1 } else { key.outline },
                    shadow: if cert { 0 } else { key.shadow },
                    mode: 0,
                    enlarge: cert,
                    wear: key.wear,
                },
                depth + 1,
            )?)
        } else {
            None
        };
        // Math.random varies only when the original sprite cache misses.
        let light = std::array::from_fn(|_| {
            ((self.random.next_int() as u32 as f64 / 4294967296.) / 10.) as f32 + 0.95
        });
        if key.wear {
            if let Some(p) = self.palette {
                obj.inventory.recol_palette.truncate(obj.recol_s.len());
                for (src, dst) in p.recolor_pairs(&self.defaults) {
                    obj.recol_s.push(src as u16);
                    obj.recol_d.push(dst as u16);
                }
                for (src, dst) in p.retexture_pairs(&self.defaults) {
                    obj.retex_s.push(src as u16);
                    obj.retex_d.push(dst as u16);
                }
            }
        }
        let mut pixels = self
            .models
            .render(&self.pack, &obj, key.outline, key.enlarge, light)?;
        if key.outline >= 1 {
            pixels = outline(&pixels, 0xff000002u32 as i32);
        }
        if key.outline >= 2 {
            pixels = outline(&pixels, -1);
        }
        if key.shadow != 0 {
            for y in (1..32).rev() {
                for x in (1..36).rev() {
                    let i = y * 36 + x;
                    if pixels[i] == 0 && pixels[i - 37] != 0 {
                        pixels[i] = key.shadow;
                    }
                }
            }
        }
        if let (Some((kind, _)), Some(over)) = (overlay, over) {
            if kind == 1 || kind == 2 {
                let mut under = over.argb.clone();
                composite(&mut under, &pixels);
                pixels = under;
            } else {
                composite(&mut pixels, &over.argb);
            }
        }
        if key.mode == 1
            || key.mode == 2 && (obj.inventory.stackable == 1 || key.count != 1) && key.count != -1
        {
            let (value, colour) = if key.count < 100000 {
                (key.count.to_string(), self.colours[0])
            } else if key.count < 10000000 {
                (format!("{}K", key.count / 1000), self.colours[1])
            } else {
                (format!("{}M", key.count / 1000000), self.colours[2])
            };
            self.count(&mut pixels, &value, colour)?;
        }
        for p in &mut pixels {
            *p = if *p & 0xffffff == 0 {
                0
            } else {
                *p | 0xff000000u32 as i32
            };
        }
        let sprite = Rc::new(Sprite {
            paletted: None,
            size: [36, 32],
            padding: [0; 4],
            argb: pixels,
        });
        if self.cache.len() >= 250 {
            if let Some(old) = self.order.pop_front() {
                self.cache.remove(&old);
            }
        }
        self.order.push_back(key);
        self.cache.insert(key, sprite.clone());
        Ok(sprite)
    }
    fn count(&self, pixels: &mut [i32], text: &str, colour: i32) -> Result<()> {
        let mut draws = vec![];
        let m = &self.font.metrics;
        crate::font_layout::line(
            m,
            crate::font_layout::LineAt {
                text: &text.encode_utf16().collect::<Vec<_>>(),
                x: 0,
                baseline: 9,
            },
            &mut crate::font_layout::Style::new(
                colour | 0xff000000u32 as i32,
                0xff000001u32 as i32,
            ),
            &crate::font_layout::Images::default(),
            false,
            &mut draws,
        )?;
        for draw in draws {
            if let crate::font_layout::Draw::Glyph {
                code, x, y, colour, ..
            } = draw
            {
                let r = m.rects[code as usize];
                let scale = m.scale as i32;
                for dy in 0..r[3] as i32 / scale {
                    for dx in 0..r[2] as i32 / scale {
                        let px = x + dx;
                        let py = y + m.bearings[code as usize] as i32 + dy;
                        if !(0..36).contains(&px) || !(0..32).contains(&py) {
                            continue;
                        }
                        let sample = (r[1] as i32 + dy * scale) * self.font.atlas.width as i32
                            + r[0] as i32
                            + dx * scale;
                        let alpha = (self.font.atlas.argb[sample as usize] >> 24) as i32;
                        if alpha == 255 {
                            pixels[(py * 36 + px) as usize] = colour;
                        } else if alpha != 0 {
                            let i = (py * 36 + px) as usize;
                            pixels[i] = crate::colour::blend_argb(
                                pixels[i],
                                (alpha << 24) | (colour & 0xffffff),
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
fn composite(under: &mut [i32], over: &[i32]) {
    for (a, &b) in under.iter_mut().zip(over) {
        if b != 0 {
            *a = b;
        }
    }
}
fn outline(p: &[i32], colour: i32) -> Vec<i32> {
    p.iter()
        .enumerate()
        .map(|(i, &v)| {
            if v == 0
                && ((i % 36 > 0 && p[i - 1] != 0)
                    || (i >= 36 && p[i - 36] != 0)
                    || (i % 36 < 35 && p[i + 1] != 0)
                    || (i < 1116 && p[i + 36] != 0))
            {
                colour
            } else {
                v
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
