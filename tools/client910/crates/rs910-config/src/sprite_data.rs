//! Decoded sprite data, paletted and full-colour. Preserve palettes and alpha independently through transforms. Palette
//! references are shared between decoded frames until a palette is extended.
use anyhow::{Context, Result};
use std::{cell::RefCell, rc::Rc};
#[derive(Clone, Debug)]
pub enum Pixels {
    Paletted {
        palette: Rc<RefCell<Vec<i32>>>,
        indices: Vec<u8>,
        alpha: Option<Vec<u8>>,
    },
    Full {
        argb: Vec<i32>,
        translucent: bool,
    },
}
#[derive(Clone, Debug)]
pub struct Data {
    pub width: i32,
    pub height: i32,
    pub padding: [i32; 4],
    pub pixels: Pixels,
}
fn area(w: i32, h: i32) -> Result<usize> {
    usize::try_from(w.wrapping_mul(h)).context("negative sprite array size")
}
fn zeros<T: Default + Clone>(n: usize) -> Result<Vec<T>> {
    let mut v = vec![];
    v.try_reserve_exact(n)?;
    v.resize(n, T::default());
    Ok(v)
}
impl Data {
    pub fn decode(bytes: &[u8]) -> Result<Vec<Self>> {
        use native910::sprite::{decode_sprite, FullSheet, PalettedSheet, SpriteSheet};
        match decode_sprite(bytes)? {
            SpriteSheet::Paletted(PalettedSheet {
                canvas_width,
                canvas_height,
                palette,
                sprites,
            }) => {
                // A stored 0 entry draws as 1; index 0 is transparent.
                let palette: Vec<i32> = palette
                    .iter()
                    .enumerate()
                    .map(|(i, &rgb)| if i == 0 { 0 } else { rgb.max(1) as i32 })
                    .collect();
                let palette = Rc::new(RefCell::new(palette));
                let (cw, ch) = (i32::from(canvas_width), i32::from(canvas_height));
                Ok(sprites
                    .into_iter()
                    .map(|sprite| {
                        let (width, height) = (i32::from(sprite.width), i32::from(sprite.height));
                        let (left, top) = (
                            i32::from(sprite.padding_left),
                            i32::from(sprite.padding_top),
                        );
                        Self {
                            width,
                            height,
                            padding: [left, top, cw - width - left, ch - height - top],
                            pixels: Pixels::Paletted {
                                palette: palette.clone(),
                                indices: sprite.colour,
                                // An all-opaque alpha plane is dropped.
                                alpha: sprite.alpha.filter(|a| a.iter().any(|v| *v != 255)),
                            },
                        }
                    })
                    .collect())
            }
            SpriteSheet::Full(FullSheet {
                width,
                height,
                sprites,
                ..
            }) => Ok(sprites
                .into_iter()
                .map(|sprite| {
                    let argb: Vec<i32> = sprite
                        .rgb
                        .chunks_exact(3)
                        .enumerate()
                        .map(|(i, c)| {
                            let rgb =
                                i32::from(c[0]) << 16 | i32::from(c[1]) << 8 | i32::from(c[2]);
                            // Magenta is transparent; an alpha plane then
                            // replaces the alpha byte.
                            let p = if rgb == 0xff00ff {
                                0
                            } else {
                                rgb | 0xff000000u32 as i32
                            };
                            sprite
                                .alpha
                                .as_ref()
                                .map_or(p, |a| (p & 0xffffff) | i32::from(a[i]) << 24)
                        })
                        .collect();
                    let translucent = argb.iter().any(|p| *p as u32 >> 24 != 255);
                    Self {
                        width: i32::from(width),
                        height: i32::from(height),
                        padding: [0; 4],
                        pixels: Pixels::Full { argb, translucent },
                    }
                })
                .collect()),
        }
    }
    pub fn full_size(&self) -> [i32; 2] {
        [
            self.padding[0]
                .wrapping_add(self.width)
                .wrapping_add(self.padding[2]),
            self.padding[1]
                .wrapping_add(self.height)
                .wrapping_add(self.padding[3]),
        ]
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite ops without a production caller yet; tested"
        )
    )]
    pub fn translucent(&self) -> bool {
        match &self.pixels {
            Pixels::Paletted { alpha, .. } => alpha.is_some(),
            Pixels::Full { translucent, .. } => *translucent,
        }
    }
    /// Pixel at `(x, y)`; ignores the paletted alpha plane. Full sprites return ARGB.
    pub fn pixel(&self, x: i32, y: i32) -> Result<i32> {
        let at = self.width.wrapping_mul(y).wrapping_add(x) as usize;
        match &self.pixels {
            Pixels::Paletted {
                palette, indices, ..
            } => Ok(*palette
                .borrow()
                .get(*indices.get(at).context("sprite pixel index")? as usize)
                .context("sprite palette index")?),
            Pixels::Full { argb, .. } => Ok(*argb.get(at).context("sprite pixel index")?),
        }
    }
    pub fn flip(&mut self, vertical: bool) {
        let w = self.width as usize;
        let h = self.height as usize;
        fn flip<T>(v: &mut [T], w: usize, h: usize, vertical: bool) {
            if vertical {
                for y in 0..h / 2 {
                    for x in 0..w {
                        v.swap(y * w + x, (h - y - 1) * w + x);
                    }
                }
            } else {
                for y in 0..h {
                    for x in 0..w / 2 {
                        v.swap(y * w + x, y * w + w - x - 1);
                    }
                }
            }
        }
        match &mut self.pixels {
            Pixels::Paletted { indices, alpha, .. } => {
                flip(indices, w, h, vertical);
                if let Some(a) = alpha {
                    flip(a, w, h, vertical);
                }
            }
            Pixels::Full { argb, .. } => flip(argb, w, h, vertical),
        }
        if vertical {
            self.padding.swap(1, 3);
        } else {
            self.padding.swap(0, 2);
        }
    }
    pub fn rotate(&mut self) {
        let w = self.width as usize;
        let h = self.height as usize;
        fn rotate<T: Copy>(v: &mut Vec<T>, w: usize, h: usize) {
            let mut out = Vec::with_capacity(v.len());
            for x in 0..w {
                for y in (0..h).rev() {
                    out.push(v[y * w + x]);
                }
            }
            *v = out;
        }
        match &mut self.pixels {
            Pixels::Paletted { indices, alpha, .. } => {
                rotate(indices, w, h);
                if let Some(a) = alpha {
                    rotate(a, w, h);
                }
            }
            Pixels::Full { argb, .. } => rotate(argb, w, h),
        }
        let [l, t, r, b] = self.padding;
        self.padding = [b, l, t, r];
        std::mem::swap(&mut self.width, &mut self.height);
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite ops without a production caller yet; tested"
        )
    )]
    pub fn clear_padding(&mut self) {
        self.padding = [0; 4];
    }
    /// Expand into available padding. A full sprite's translucency
    /// flag scans the old pixels, not the newly introduced transparent border.
    pub fn expand(&mut self, n: i32) -> Result<()> {
        let [fw, fh] = self.full_size();
        if self.width == fw && self.height == fh {
            return Ok(());
        }
        let [left, top, _, _] = self.padding;
        let l = n.min(left);
        let t = n.min(top);
        let r = if left.wrapping_add(n).wrapping_add(self.width) > fw {
            fw.wrapping_sub(left).wrapping_sub(self.width)
        } else {
            n
        };
        let b = if top.wrapping_add(n).wrapping_add(self.height) > fh {
            fh.wrapping_sub(top).wrapping_sub(self.height)
        } else {
            n
        };
        let w = self.width.wrapping_add(l).wrapping_add(r);
        let h = self.height.wrapping_add(t).wrapping_add(b);
        fn expand<T: Copy + Default + Clone>(
            v: &[T],
            old: [i32; 2],
            w: i32,
            h: i32,
            l: i32,
            t: i32,
        ) -> Result<Vec<T>> {
            let mut result = zeros(area(w, h)?)?;
            for y in 0..old[1] {
                for x in 0..old[0] {
                    let from = old[0].wrapping_mul(y).wrapping_add(x);
                    let to = y
                        .wrapping_add(t)
                        .wrapping_mul(w)
                        .wrapping_add(l)
                        .wrapping_add(x);
                    *result
                        .get_mut(to as usize)
                        .context("expanded sprite index")? =
                        *v.get(from as usize).context("source sprite index")?;
                }
            }
            Ok(result)
        }
        let old = [self.width, self.height];
        match &mut self.pixels {
            Pixels::Paletted { indices, alpha, .. } => {
                let v = expand(indices, old, w, h, l, t)?;
                if let Some(a) = alpha {
                    *a = expand(a, old, w, h, l, t)?;
                }
                *indices = v;
            }
            Pixels::Full { argb, translucent } => {
                let mut v = zeros(area(w, h)?)?;
                *translucent = false;
                for y in 0..old[1] {
                    for x in 0..old[0] {
                        let from = old[0].wrapping_mul(y).wrapping_add(x);
                        let to = y
                            .wrapping_add(t)
                            .wrapping_mul(w)
                            .wrapping_add(l)
                            .wrapping_add(x);
                        let p = *argb.get(from as usize).context("source sprite index")?;
                        if p as u32 >> 24 != 255 {
                            *translucent = true;
                        }
                        *v.get_mut(to as usize).context("expanded sprite index")? = p;
                    }
                }
                *argb = v;
            }
        }
        for (p, n) in self.padding.iter_mut().zip([l, t, r, b]) {
            *p = p.wrapping_sub(n);
        }
        self.width = w;
        self.height = h;
        Ok(())
    }
    fn palette_colour(palette: &mut Rc<RefCell<Vec<i32>>>, colour: i32) -> usize {
        let p = palette.borrow();
        if p.len() < 255 {
            if let Some(n) = p.iter().position(|v| *v == colour) {
                return n;
            }
            let mut next = p.clone();
            let n = next.len();
            next.push(colour);
            drop(p);
            *palette = Rc::new(RefCell::new(next));
            n
        } else {
            let rgb = |v: i32| [(v >> 16) & 255, (v >> 8) & 255, v & 255];
            let target = rgb(colour);
            p.iter()
                .enumerate()
                .min_by_key(|(_, v)| {
                    rgb(**v)
                        .iter()
                        .zip(target)
                        .map(|(a, b)| (a - b).abs())
                        .sum::<i32>()
                })
                .unwrap()
                .0
        }
    }
    pub fn outline(&mut self, colour: i32) -> Result<()> {
        let w = self.width as usize;
        let h = self.height as usize;
        match &mut self.pixels {
            Pixels::Paletted {
                palette, indices, ..
            } => {
                let fill = Self::palette_colour(palette, colour);
                let p = palette.borrow();
                let mut next = zeros(w * h)?;
                let solid = |i: usize| -> Result<bool> {
                    Ok(*p
                        .get(indices[i] as usize)
                        .context("outline palette index")?
                        != 0)
                };
                for y in 0..h {
                    for x in 0..w {
                        let i = y * w + x;
                        let mut v = indices[i];
                        if !solid(i)?
                            && ((x > 0 && solid(i - 1)?)
                                || (y > 0 && solid(i - w)?)
                                || (x + 1 < w && solid(i + 1)?)
                                || (y + 1 < h && solid(i + w)?))
                        {
                            v = fill as u8;
                        }
                        next[i] = v;
                    }
                }
                *indices = next;
            }
            Pixels::Full { argb, translucent } => {
                *translucent = false;
                let mut next = zeros(w * h)?;
                let solid = |i: usize| argb[i] as u32 >> 24 != 0;
                for y in 0..h {
                    for x in 0..w {
                        let i = y * w + x;
                        let mut v = argb[i];
                        if !solid(i)
                            && ((x > 0 && solid(i - 1))
                                || (y > 0 && solid(i - w))
                                || (x + 1 < w && solid(i + 1))
                                || (y + 1 < h && solid(i + w)))
                        {
                            v = colour;
                        }
                        if v as u32 >> 24 != 255 {
                            *translucent = true;
                        }
                        next[i] = v;
                    }
                }
                *argb = next;
            }
        }
        Ok(())
    }
    pub fn shadow(&mut self, colour: i32) -> Result<()> {
        let w = self.width as usize;
        let h = self.height as usize;
        match &mut self.pixels {
            Pixels::Paletted {
                palette, indices, ..
            } => {
                let fill = Self::palette_colour(palette, colour);
                let p = palette.borrow();
                for y in (1..h).rev() {
                    for x in (1..w).rev() {
                        let i = y * w + x;
                        if *p.get(indices[i] as usize).context("shadow palette index")? == 0
                            && *p
                                .get(indices[i - w - 1] as usize)
                                .context("shadow palette index")?
                                != 0
                        {
                            indices[i] = fill as u8;
                        }
                    }
                }
            }
            Pixels::Full { argb, .. } => {
                for y in (1..h).rev() {
                    for x in (1..w).rev() {
                        let i = y * w + x;
                        if argb[i] as u32 >> 24 == 0 && argb[i - w - 1] as u32 >> 24 != 0 {
                            argb[i] = colour;
                        }
                    }
                }
            }
        }
        Ok(())
    }
    #[cfg_attr(
        not(test),
        allow(
            dead_code,
            reason = "sprite ops without a production caller yet; tested"
        )
    )]
    pub fn recolour(&mut self, delta: [i32; 3]) {
        let rgb = |v: i32| {
            let ch = [(v >> 16) & 255, (v >> 8) & 255, v & 255];
            let c = std::array::from_fn::<_, 3, _>(|i| ch[i].wrapping_add(delta[i]).clamp(0, 255));
            c[0] << 16 | c[1] << 8 | c[2]
        };
        match &mut self.pixels {
            Pixels::Paletted { palette, .. } => {
                for p in palette.borrow_mut().iter_mut().skip(1) {
                    if *p != 1 && *p != 0xff00ff {
                        *p = rgb(*p);
                    }
                }
            }
            Pixels::Full { argb, .. } => {
                for p in argb.iter_mut().skip(1) {
                    *p = (*p & 0xff000000u32 as i32) | rgb(*p);
                }
            }
        }
    }
    pub fn argb(&self, padded: bool) -> Result<Vec<i32>> {
        let [fw, fh] = self.full_size();
        if let Pixels::Full { argb, .. } = &self.pixels {
            if !padded || (fw == self.width && fh == self.height) {
                return Ok(argb.clone());
            }
        }
        let (w, h, l, t) = if padded {
            (fw, fh, self.padding[0], self.padding[1])
        } else {
            (self.width, self.height, 0, 0)
        };
        let mut result = zeros(area(w, h)?)?;
        for y in 0..self.height {
            for x in 0..self.width {
                let i = (y * self.width + x) as usize;
                let v = match &self.pixels {
                    Pixels::Paletted {
                        palette,
                        indices,
                        alpha,
                    } => {
                        let rgb = *palette
                            .borrow()
                            .get(indices[i] as usize)
                            .context("sprite palette index")?;
                        if let Some(a) = alpha {
                            ((a[i] as i32) << 24) | rgb
                        } else if rgb == 0 {
                            0
                        } else {
                            rgb | 0xff000000u32 as i32
                        }
                    }
                    Pixels::Full { argb, .. } => argb[i],
                };
                *result
                    .get_mut(
                        y.wrapping_add(t)
                            .wrapping_mul(w)
                            .wrapping_add(l)
                            .wrapping_add(x) as usize,
                    )
                    .context("padded sprite index")? = v;
            }
        }
        Ok(result)
    }
}
