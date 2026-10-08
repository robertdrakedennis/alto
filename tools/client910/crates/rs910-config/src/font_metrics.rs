//! Font metrics, retaining the atlas data consumed by the GPU font atlas.
//! Byte arrays stay signed for kerning arithmetic and unsigned for glyph
//! dimensions. Strings are UTF-16; the raw-char kerning indices used for
//! string width are deliberately distinct from its Cp1252 advance indices.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Metrics {
    pub advances: [u8; 256],
    /// Glyph heights (the legacy interface field was named `widths`).
    pub widths: [u8; 256],
    /// Vertical offsets (the legacy interface field was named `bearings`).
    pub bearings: [u8; 256],
    pub kerning: Option<[[i8; 256]; 256]>,
    /// Default line height; not the advance of the space character.
    pub space_width: i32,
    pub ascent: i32,
    pub descent: i32,
    /// Two further per-font metrics, scaled like the others; layout does not
    /// use them, they are only handed to scripts by the font-metrics query.
    pub metric_a: i32,
    pub metric_b: i32,
    pub scale: u8,
    pub atlas_width: u16,
    pub atlas_height: u16,
    /// Unscaled atlas rectangles. Width/height are sign-extended bytes.
    pub rects: [[i16; 4]; 256],
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            advances: [0; 256],
            widths: [0; 256],
            bearings: [0; 256],
            kerning: None,
            space_width: 0,
            ascent: 0,
            descent: 0,
            metric_a: 0,
            metric_b: 0,
            scale: 1,
            atlas_width: 0,
            atlas_height: 0,
            rects: [[0; 4]; 256],
        }
    }
}

impl Metrics {
    /// Decode the metrics. Invalid inputs fail as in the original
    /// (unsupported version, negative profile size, division by zero or EOF).
    pub fn decode(data: &[u8]) -> anyhow::Result<Self> {
        let mut at = 0;
        let mut byte = || -> anyhow::Result<u8> {
            let b = *data
                .get(at)
                .ok_or_else(|| anyhow::anyhow!("font metrics EOF"))?;
            at += 1;
            Ok(b)
        };
        anyhow::ensure!(byte()? == 0, "font metrics version");
        let has_kerning = byte()? == 1;
        let mut m = Self::default();
        for a in [&mut m.advances, &mut m.widths, &mut m.bearings] {
            for v in a {
                *v = byte()?;
            }
        }
        m.atlas_width = u16::from_be_bytes([byte()?, byte()?]);
        m.atlas_height = u16::from_be_bytes([byte()?, byte()?]);
        for axis in 0..2 {
            for rect in &mut m.rects {
                rect[axis] = i16::from_be_bytes([byte()?, byte()?]);
            }
        }
        for (i, r) in m.rects.iter_mut().enumerate() {
            r[2] = m.advances[i] as i8 as i16;
            r[3] = m.widths[i] as i8 as i16;
        }
        if has_kerning {
            let mut planes = [Vec::new(), Vec::new()];
            for plane in &mut planes {
                for &height in &m.widths {
                    anyhow::ensure!((height as i8) >= 0, "negative font profile height");
                    let mut row = Vec::with_capacity(height as usize);
                    let mut value = 0i8;
                    for _ in 0..height {
                        value = value.wrapping_add(byte()? as i8);
                        row.push(value);
                    }
                    plane.push(row);
                }
            }
            let mut table = [[0; 256]; 256];
            for a in 0..256 {
                if a == 32 || a == 160 {
                    continue;
                }
                for b in 0..256 {
                    if b == 32 || b == 160 {
                        continue;
                    }
                    let ya = m.bearings[a] as i8 as i32;
                    let yb = m.bearings[b] as i8 as i32;
                    let start = ya.max(yb);
                    let end = (ya + m.widths[a] as i8 as i32).min(yb + m.widths[b] as i8 as i32);
                    let mut gap = m.advances[a].min(m.advances[b]) as i32;
                    for y in start..end {
                        gap = gap.min(
                            planes[1][a][(y - ya) as usize] as i32
                                + planes[0][b][(y - yb) as usize] as i32,
                        );
                    }
                    table[a][b] = (-gap) as i8;
                }
            }
            m.kerning = Some(table);
            m.space_width = m.bearings[32] as i8 as i32 + m.widths[32] as i8 as i32;
        } else {
            m.space_width = byte()? as i32;
        }
        m.metric_a = byte()? as i32;
        m.metric_b = byte()? as i32;
        m.ascent = byte()? as i32;
        m.descent = byte()? as i32;
        m.scale = byte()?;
        anyhow::ensure!(m.scale != 0, "font metrics scale zero");
        if m.scale != 1 {
            let d = m.scale as i32;
            for v in [
                &mut m.space_width,
                &mut m.metric_a,
                &mut m.metric_b,
                &mut m.ascent,
                &mut m.descent,
            ] {
                *v /= d;
            }
            for a in [&mut m.advances, &mut m.widths, &mut m.bearings] {
                for v in a {
                    *v = (*v as i8 as i32 / d) as u8;
                }
            }
            if let Some(table) = &mut m.kerning {
                for row in table {
                    for v in row {
                        *v = (*v as i32 / d) as i8;
                    }
                }
            }
        }
        Ok(m)
    }

    /// The string width. Existing callers retain the original's failure
    /// for a raw UTF-16 char outside the kerning array; hosts can use the checked
    /// entry point to report it without unwinding a render loop.
    #[cfg(test)] // test-only helper
    pub fn string_width(&self, text: &str) -> i32 {
        self.width_utf16(&text.encode_utf16().collect::<Vec<_>>(), None)
            .expect("kerning index out of range")
    }

    /// The supplied image widths correspond to the glyph x extents.
    pub fn width_utf16(&self, text: &[u16], images: Option<&[i32]>) -> anyhow::Result<i32> {
        self.width_with_icons(text, images, None)
    }

    pub fn width_with_icons(
        &self,
        text: &[u16],
        images: Option<&[i32]>,
        icons: Option<&dyn Fn(i32) -> i32>,
    ) -> anyhow::Result<i32> {
        let provider = |id| Ok(icons.unwrap()(id));
        self.width_with_provider(
            text,
            images,
            icons.map(|_| &provider as &dyn Fn(i32) -> anyhow::Result<i32>),
        )
    }

    /// FontMetrics:275-288 catches icon-provider exceptions before resetting
    /// the preceding kerning character. Missing icons return zero successfully.
    pub fn width_with_provider(
        &self,
        text: &[u16],
        images: Option<&[i32]>,
        icons: Option<&dyn Fn(i32) -> anyhow::Result<i32>>,
    ) -> anyhow::Result<i32> {
        let mut open = None;
        let mut previous: Option<u16> = None;
        let mut width = 0i32;
        for (i, &unit) in text.iter().enumerate() {
            let mut ch = unit;
            if ch == b'<' as u16 {
                open = Some(i);
                continue;
            }
            if ch == b'>' as u16 {
                if let Some(start) = open.take() {
                    let tag = String::from_utf16_lossy(&text[start + 1..i]);
                    ch = match tag.as_str() {
                        "lt" => 60,
                        "gt" => 62,
                        "nbsp" => 160,
                        "shy" => 173,
                        "times" => 215,
                        "euro" => 128,
                        "copy" => 169,
                        "reg" => 174,
                        _ => {
                            if let (Some(id), Some(images)) = (tag.strip_prefix("img="), images) {
                                if let Ok(index) = id.parse::<i32>() {
                                    if let Some(w) = images.get(index as usize) {
                                        width = width.wrapping_add(*w);
                                        previous = None;
                                    }
                                }
                            }
                            if let (Some(id), Some(icons)) = (tag.strip_prefix("sprite="), icons) {
                                if let Ok(id) = id.split(',').next().unwrap().parse::<i32>() {
                                    if let Ok(w) = icons(id) {
                                        width = width.wrapping_add(w);
                                        previous = None;
                                    }
                                }
                            }
                            continue;
                        }
                    };
                }
            }
            if open.is_none() {
                width = width.wrapping_add(
                    self.advances[rs910_core::cp1252::cp1252_encode_unit(ch) as usize] as i32,
                );
                if let (Some(table), Some(prev)) = (&self.kerning, previous) {
                    let v = table
                        .get(prev as usize)
                        .and_then(|row| row.get(ch as usize))
                        .ok_or_else(|| anyhow::anyhow!("kerning index {prev}/{ch}"))?;
                    width = width.wrapping_add(*v as i32);
                }
                previous = Some(ch);
            }
        }
        Ok(width)
    }

    /// Four vertices, each [x,y,z,u,v]. UVs
    /// multiply by the reciprocal in f32, retaining the original evaluation order.
    pub fn glyph_vertices(&self, glyph: usize) -> [[f32; 5]; 4] {
        let r = self.rects[glyph];
        let sx = 1.0f32 / self.atlas_width as f32;
        let sy = 1.0f32 / self.atlas_height as f32;
        let u0 = r[0] as f32 * sx;
        let v0 = r[1] as f32 * sy;
        let u1 = (r[0] as i32 + r[2] as i32) as f32 * sx;
        let v1 = (r[1] as i32 + r[3] as i32) as f32 * sy;
        let w = self.advances[glyph] as f32;
        let h = self.widths[glyph] as f32;
        [
            [0., 0., 0., u0, v0],
            [0., h, 0., u0, v1],
            [w, h, 0., u1, v1],
            [w, 0., 0., u1, v0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `string_width` over synthetic metrics with hand-computed widths:
    /// markup tags add nothing, spaces use their own advance, and a kerning
    /// pair adjusts the advance.
    #[test]
    fn font_advance_goldens() {
        let mut advances = [10_u8; 256];
        advances[usize::from(b' ')] = 4;
        advances[usize::from(b'i')] = 5;
        let data = Metrics {
            advances,
            widths: [8_u8; 256],
            bearings: [1_u8; 256],
            kerning: None,
            space_width: 4,
            ascent: 12,
            descent: 3,
            ..Default::default()
        };
        assert_eq!(data.string_width(""), 0);
        assert_eq!(data.string_width("i"), 5);
        assert_eq!(data.string_width("i i"), 5 + 4 + 5);
        assert_eq!(data.string_width("<col=ff0000>hi</col>"), 15);
        // Kerning applies.
        let mut kern = [[0_i8; 256]; 256];
        kern[usize::from(b'A')][usize::from(b'V')] = -2;
        let kerned = Metrics {
            kerning: Some(kern),
            ..data.clone()
        };
        assert_eq!(kerned.string_width("AV"), 10 + 10 - 2);
    }
}
