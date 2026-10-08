//! Decoded sprite data, paletted and full-colour. Preserve palettes and alpha independently through transforms. Palette
//! references are shared between decoded frames until a palette is extended.
use anyhow::{Context, Result};
use rs910_core::reader::{Eof, Reader as CoreReader};
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
struct Read<'a> {
    data: &'a [u8],
    at: i32,
}
impl<'a> Read<'a> {
    /// One `rs910_core::reader` read at `at` (the arithmetic
    /// lives there; a negative `at` reads nothing, as before).
    fn read<T>(
        &mut self,
        read: impl FnOnce(&mut CoreReader<'a>) -> std::result::Result<T, Eof>,
    ) -> Result<T> {
        let mut r = CoreReader::at(self.data, self.at as usize);
        let out = read(&mut r);
        self.at = r.pos() as i32;
        out.map_err(|_| anyhow::anyhow!("sprite EOF"))
    }
    fn g1(&mut self) -> Result<u8> {
        self.read(CoreReader::g1)
    }
    fn g2(&mut self) -> Result<i32> {
        self.read(CoreReader::g2).map(i32::from)
    }
    fn g3(&mut self) -> Result<i32> {
        self.read(CoreReader::g3).map(|v| v as i32)
    }
}
fn plane(r: &mut Read<'_>, w: i32, h: i32, column: bool) -> Result<Vec<u8>> {
    let mut v = zeros(area(w, h)?)?;
    if column {
        for x in 0..w {
            for y in 0..h {
                v[(w * y + x) as usize] = r.g1()?;
            }
        }
    } else {
        for b in &mut v {
            *b = r.g1()?;
        }
    }
    Ok(v)
}
impl Data {
    pub fn decode(bytes: &[u8]) -> Result<Vec<Self>> {
        let mut r = Read {
            data: bytes,
            at: (bytes.len() as i32).wrapping_sub(2),
        };
        let tail = r.g2()?;
        let count = (tail & 32767) as usize;
        if tail >> 15 == 0 {
            r.at = (bytes.len() as i32)
                .wrapping_sub(7)
                .wrapping_sub(count as i32 * 8);
            let cw = r.g2()?;
            let ch = r.g2()?;
            let colours = r.g1()? as usize + 1;
            let mut dims = vec![[0; 4]; count];
            for a in 0..4 {
                for d in &mut dims {
                    d[a] = r.g2()?;
                }
            }
            r.at = (bytes.len() as i32)
                .wrapping_sub(7)
                .wrapping_sub(count as i32 * 8)
                .wrapping_sub((colours as i32 - 1) * 3);
            let mut p = vec![0; colours];
            for v in &mut p[1..] {
                *v = r.g3()?;
                if *v == 0 {
                    *v = 1;
                }
            }
            let palette = Rc::new(RefCell::new(p));
            r.at = 0;
            let mut result = vec![];
            for [left, top, width, height] in dims {
                let flags = r.g1()?;
                let indices = plane(&mut r, width, height, flags & 1 != 0)?;
                let alpha = if flags & 2 != 0 {
                    let a = plane(&mut r, width, height, flags & 1 != 0)?;
                    a.iter().any(|v| *v != 255).then_some(a)
                } else {
                    None
                };
                result.push(Self {
                    width,
                    height,
                    padding: [left, top, cw - width - left, ch - height - top],
                    pixels: Pixels::Paletted {
                        palette: palette.clone(),
                        indices,
                        alpha,
                    },
                });
            }
            Ok(result)
        } else {
            r.at = 0;
            anyhow::ensure!(r.g1()? == 0, "unsupported full sprite format");
            let alpha = r.g1()? == 1;
            let width = r.g2()?;
            let height = r.g2()?;
            let mut result = vec![];
            for _ in 0..count {
                let mut argb = zeros(area(width, height)?)?;
                for p in &mut argb {
                    let rgb = r.g3()?;
                    *p = if rgb == 0xff00ff {
                        0
                    } else {
                        rgb | 0xff000000u32 as i32
                    };
                }
                if alpha {
                    for p in &mut argb {
                        *p = (*p & 0xffffff) | ((r.g1()? as i32) << 24);
                    }
                }
                let translucent = argb.iter().any(|p| *p as u32 >> 24 != 255);
                result.push(Self {
                    width,
                    height,
                    padding: [0; 4],
                    pixels: Pixels::Full { argb, translucent },
                });
            }
            Ok(result)
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
