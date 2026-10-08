//! Coordinates and
//! retained strings use 32-bit integers and UTF-16; the terminal draw records
//! carry identity and painter order for the shared graphics consumer.
use crate::font_metrics::Metrics;
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: i32,
    pub height: i32,
}
#[derive(Clone, Default)]
pub struct Images {
    pub images: Option<Vec<Image>>,
    pub baselines: Option<Vec<i32>>,
    pub icons: Option<BTreeMap<i32, Vec<Image>>>,
}
impl Images {
    fn widths(&self) -> Option<Vec<i32>> {
        self.images
            .as_ref()
            .map(|v| v.iter().map(|i| i.width).collect())
    }
    fn icon_width(&self, id: i32) -> i32 {
        self.icons
            .as_ref()
            .and_then(|m| m.get(&id))
            .and_then(|v| v.first())
            .map_or(0, |i| i.width)
    }
    #[cfg(any(test, feature = "test-hooks"))] // test-only helper
    pub fn width(&self, m: &Metrics, text: &[u16]) -> anyhow::Result<i32> {
        let icon = |id| self.icon_width(id);
        m.width_with_icons(
            text,
            self.widths().as_deref(),
            self.icons.as_ref().map(|_| &icon as &dyn Fn(i32) -> i32),
        )
    }
}

/// Synchronous font-icon boundary. Layout and drawing consult the same
/// provider at their original call sites; missing resources are retryable.
pub trait IconProvider {
    fn width(&self, id: i32) -> anyhow::Result<i32>;
    fn frames(&self, id: i32) -> anyhow::Result<Option<std::rc::Rc<Vec<Image>>>>;
}
impl IconProvider for Images {
    fn width(&self, id: i32) -> anyhow::Result<i32> {
        Ok(self.icon_width(id))
    }
    fn frames(&self, id: i32) -> anyhow::Result<Option<std::rc::Rc<Vec<Image>>>> {
        Ok(self
            .icons
            .as_ref()
            .and_then(|v| v.get(&id))
            .map(|v| std::rc::Rc::new(v.clone())))
    }
}
/// GPU consumers execute each terminal immediately, before the next resource
/// lookup. A captured Vec remains useful for geometry/oracle consumers.
pub trait Sink {
    fn emit(&mut self, draw: Draw) -> anyhow::Result<()>;
}
impl Sink for Vec<Draw> {
    fn emit(&mut self, draw: Draw) -> anyhow::Result<()> {
        self.push(draw);
        Ok(())
    }
}
impl Images {
    fn provider(&self) -> Option<&dyn IconProvider> {
        self.icons.as_ref().map(|_| self as &dyn IconProvider)
    }
    fn measure(
        &self,
        m: &Metrics,
        text: &[u16],
        inline: bool,
        provider: Option<&dyn IconProvider>,
    ) -> anyhow::Result<i32> {
        let widths = if inline { self.widths() } else { None };
        let width = |id| provider.unwrap().width(id);
        m.width_with_provider(
            text,
            widths.as_deref(),
            provider.map(|_| &width as &dyn Fn(i32) -> anyhow::Result<i32>),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draw {
    Glyph {
        code: u8,
        x: i32,
        y: i32,
        colour: i32,
        shadow: bool,
        masked: bool,
    },
    Line {
        x: i32,
        y: i32,
        width: i32,
        colour: i32,
    },
    Image {
        icon: bool,
        id: i32,
        frame: i32,
        x: i32,
        y: i32,
        mode: i32,
        colour: i32,
    },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Style {
    pub strike: i32,
    pub underline: i32,
    pub original: i32,
    pub colour: i32,
    pub original_shadow: i32,
    pub shadow: i32,
    pub spacing: i32,
    pub remainder: i32,
}
impl Default for Style {
    fn default() -> Self {
        Self::new(0, 0)
    }
}
impl Style {
    pub fn new(colour: i32, shadow: i32) -> Self {
        let shadow = if shadow == -1 { 0 } else { shadow };
        Self {
            strike: -1,
            underline: -1,
            original: colour,
            colour,
            original_shadow: shadow,
            shadow,
            spacing: 0,
            remainder: 0,
        }
    }
    /// Font.evaluateTag:269-305; signed radix parsing rejects positive values
    /// above i32::MAX, and malformed tags leave all preceding changes intact.
    pub fn tag(&mut self, t: &str) {
        let apply = |s: &mut Self| -> Result<(), std::num::ParseIntError> {
            let hex = |v: &str| i32::from_str_radix(v, 16);
            if let Some(v) = t.strip_prefix("col=") {
                s.colour = (s.colour & !0xffffff) | (hex(v)? & 0xffffff);
            } else if t == "/col" {
                s.colour = (s.colour & !0xffffff) | (s.original & 0xffffff);
            }
            if let Some(v) = t.strip_prefix("argb=") {
                s.colour = hex(v)?;
            } else if t == "/argb" {
                s.colour = s.original;
            } else if let Some(v) = t.strip_prefix("str=") {
                s.strike = (s.colour & !0xffffff) | hex(v)?;
            } else if t == "str" {
                s.strike = (s.colour & !0xffffff) | 0x800000;
            } else if t == "/str" {
                s.strike = -1;
            } else if let Some(v) = t.strip_prefix("u=") {
                s.underline = (s.colour & !0xffffff) | hex(v)?;
            } else if t == "u" {
                s.underline = s.colour & !0xffffff;
            } else if t == "/u" {
                s.underline = -1;
            } else if t.eq_ignore_ascii_case("shad=-1") {
                s.shadow = 0;
            } else if let Some(v) = t.strip_prefix("shad=") {
                s.shadow = (s.colour & !0xffffff) | hex(v)?;
            } else if t == "shad" {
                s.shadow = s.colour & !0xffffff;
            } else if t == "/shad" {
                s.shadow = s.original_shadow;
            } else if t == "br" {
                *s = Self::new(s.original, s.original_shadow);
            }
            Ok(())
        };
        let _ = apply(self);
    }
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "layout without a production caller yet; tested")
    )]
    pub fn justify(
        &mut self,
        m: &Metrics,
        text: &[u16],
        width: i32,
        images: &Images,
    ) -> anyhow::Result<()> {
        self.justify_with_provider(m, text, width, images, images.provider())
    }
    pub fn justify_with_provider(
        &mut self,
        m: &Metrics,
        text: &[u16],
        width: i32,
        images: &Images,
        provider: Option<&dyn IconProvider>,
    ) -> anyhow::Result<()> {
        let (mut tags, mut spaces) = (false, 0);
        for &c in text {
            if c == 60 {
                tags = true;
            } else if c == 62 {
                tags = false;
            } else if !tags && c == 32 {
                spaces += 1;
            }
        }
        if spaces > 0 {
            self.spacing = width
                .wrapping_sub(images.measure(m, text, false, provider)?)
                .wrapping_shl(8)
                / spaces;
        }
        Ok(())
    }
}
fn entity(tag: &str) -> Option<u16> {
    Some(match tag {
        "lt" => 60,
        "gt" => 62,
        "nbsp" => 160,
        "shy" => 173,
        "times" => 215,
        "euro" => 128,
        "copy" => 169,
        "reg" => 174,
        _ => return None,
    })
}
fn kern(m: &Metrics, previous: Option<u16>, c: u16) -> anyhow::Result<i32> {
    match (&m.kerning, previous) {
        (Some(k), Some(p)) => k
            .get(p as usize)
            .and_then(|r| r.get(c as usize))
            .map(|v| *v as i32)
            .ok_or_else(|| anyhow::anyhow!("font kerning index")),
        _ => Ok(0),
    }
}

/// One line of text and where it sits: its start `x` and its baseline.
#[derive(Clone, Copy)]
pub struct LineAt<'a> {
    pub text: &'a [u16],
    pub x: i32,
    pub baseline: i32,
}

/// What inline images and icons resolve against while laying out text.
#[derive(Clone, Copy)]
pub struct TextResources<'a> {
    pub images: &'a Images,
    pub provider: Option<&'a dyn IconProvider>,
}

/// Lays out one line of text into `out`. The baseline subtraction precedes
/// the terminal glyph vertical offset; shadows and decorations keep their
/// exact per-character order.
pub fn line(
    m: &Metrics,
    at: LineAt<'_>,
    style: &mut Style,
    images: &Images,
    masked: bool,
    out: &mut Vec<Draw>,
) -> anyhow::Result<()> {
    line_to(
        m,
        at,
        style,
        TextResources {
            images,
            provider: images.provider(),
        },
        masked,
        out,
    )
}
/// [`line`] into any [`Sink`], with an explicit icon provider.
pub fn line_to(
    m: &Metrics,
    at: LineAt<'_>,
    style: &mut Style,
    resources: TextResources<'_>,
    masked: bool,
    out: &mut dyn Sink,
) -> anyhow::Result<()> {
    line_impl(m, at, style, resources, masked, None, out)
}
/// Per-character offsets for the jittered line forms.
#[derive(Clone, Copy)]
pub struct Offsets<'a> {
    pub x: Option<&'a [i32]>,
    pub y: Option<&'a [i32]>,
}
fn offset(offsets: Option<Offsets<'_>>, at: &mut usize) -> anyhow::Result<[i32; 2]> {
    let Some(o) = offsets else { return Ok([0, 0]) };
    let get = |a: Option<&[i32]>| -> anyhow::Result<i32> {
        match a {
            None => Ok(0),
            Some(v) => v
                .get(*at)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("font offset index")),
        }
    };
    let v = [get(o.x)?, get(o.y)?];
    *at += 1;
    Ok(v)
}
/// The alpha line form has its own image alpha, icon baseline and underline
/// rules. Offset consumption includes invalid image tags.
pub fn alpha_line_to(
    m: &Metrics,
    at: LineAt<'_>,
    style: &mut Style,
    resources: TextResources<'_>,
    offsets: Offsets<'_>,
    out: &mut dyn Sink,
) -> anyhow::Result<()> {
    line_impl(m, at, style, resources, false, Some(offsets), out)
}
/// The shared body of the plain and alpha line forms.
fn line_impl(
    m: &Metrics,
    at: LineAt<'_>,
    style: &mut Style,
    resources: TextResources<'_>,
    masked: bool,
    offsets: Option<Offsets<'_>>,
    out: &mut dyn Sink,
) -> anyhow::Result<()> {
    let LineAt {
        text,
        x: start_x,
        baseline,
    } = at;
    let mut x = start_x;
    let TextResources { images, provider } = resources;
    let y = baseline.wrapping_sub(m.space_width);
    let (mut open, mut previous, mut at) = (None, None, 0);
    for (i, &unit) in text.iter().enumerate() {
        let mut c = rs910_core::cp1252::cp1252_encode_unit(unit) as u16;
        if c == 60 {
            open = Some(i);
            continue;
        }
        if c == 62 {
            if let Some(start) = open.take() {
                let tag = String::from_utf16_lossy(&text[start + 1..i]);
                if let Some(ch) = entity(&tag) {
                    c = ch;
                } else {
                    if let Some(v) = tag.strip_prefix("img=") {
                        let _ = (|| -> anyhow::Result<()> {
                            let [dx, dy] = offset(offsets, &mut at)?;
                            let id = v.parse::<i32>()?;
                            let image = images
                                .images
                                .as_ref()
                                .and_then(|v| v.get(id as usize))
                                .ok_or_else(|| anyhow::anyhow!("inline image index"))?;
                            let height = match &images.baselines {
                                None => image.height,
                                Some(v) => *v
                                    .get(id as usize)
                                    .ok_or_else(|| anyhow::anyhow!("inline baseline index"))?,
                            };
                            let (mode, colour) =
                                if offsets.is_some() || style.colour & !0xffffff == !0xffffff {
                                    (1, -1)
                                } else {
                                    (0, (style.colour & !0xffffff) | 0xffffff)
                                };
                            out.emit(Draw::Image {
                                icon: false,
                                id,
                                frame: 0,
                                x: x.wrapping_add(dx),
                                y: baseline.wrapping_sub(height).wrapping_add(dy),
                                mode,
                                colour,
                            })?;
                            x = x.wrapping_add(image.width);
                            previous = None;
                            Ok(())
                        })();
                    } else if let Some(v) = tag.strip_prefix("sprite=") {
                        if let Some(provider) = provider {
                            let _ = (|| -> anyhow::Result<()> {
                                let mut parts = v.splitn(2, ',');
                                let id = parts.next().unwrap().parse::<i32>()?;
                                let frame = parts.next().unwrap_or("0").parse::<i32>()?;
                                let [dx, dy] = offset(offsets, &mut at)?;
                                if let Some(frames) = provider.frames(id)? {
                                    let image = frames
                                        .get(frame as usize)
                                        .ok_or_else(|| anyhow::anyhow!("icon frame index"))?;
                                    let height = image.height.min(m.ascent.wrapping_add(m.descent));
                                    let (mode, colour) = if offsets.is_some()
                                        || style.colour & !0xffffff == !0xffffff
                                    {
                                        (1, -1)
                                    } else {
                                        (0, (style.colour & !0xffffff) | 0xffffff)
                                    };
                                    out.emit(Draw::Image {
                                        icon: true,
                                        id,
                                        frame,
                                        x: x.wrapping_add(dx),
                                        y: baseline
                                            .wrapping_add(if offsets.is_some() { 3 } else { 2 })
                                            .wrapping_sub(height)
                                            .wrapping_add(dy),
                                        mode,
                                        colour,
                                    })?;
                                    x = x.wrapping_add(image.width);
                                }
                                previous = None;
                                Ok(())
                            })();
                        }
                    } else {
                        style.tag(&tag);
                    }
                    continue;
                }
            }
        }
        if open.is_some() {
            continue;
        }
        x = x.wrapping_add(kern(m, previous, c)?);
        let [dx, dy] = offset(offsets, &mut at)?;
        if c == 32 {
            if style.spacing > 0 {
                style.remainder = style.remainder.wrapping_add(style.spacing);
                x = x.wrapping_add(style.remainder >> 8);
                style.remainder &= 255;
            }
        } else {
            if style.shadow & !0xffffff != 0 {
                out.emit(Draw::Glyph {
                    code: c as u8,
                    x: x.wrapping_add(1).wrapping_add(dx),
                    y: y.wrapping_add(1).wrapping_add(dy),
                    colour: style.shadow,
                    shadow: true,
                    masked,
                })?;
            }
            out.emit(Draw::Glyph {
                code: c as u8,
                x: x.wrapping_add(dx),
                y: y.wrapping_add(dy),
                colour: style.colour,
                shadow: false,
                masked,
            })?;
        }
        let width = m.advances[c as usize] as i32;
        if style.strike != -1 {
            out.emit(Draw::Line {
                x,
                y: ((m.space_width as f64 * 0.7) as i32).wrapping_add(y),
                width,
                colour: style.strike,
            })?;
        }
        if style.underline != -1 {
            out.emit(Draw::Line {
                x,
                y: baseline.wrapping_add(if offsets.is_some() { 0 } else { 1 }),
                width,
                colour: style.underline,
            })?;
        }
        x = x.wrapping_add(width);
        previous = Some(c);
    }
    Ok(())
}

/// FontMetrics.truncString:297-394 retains the exact UTF-16 prefix and tags.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "FontMetrics.truncString port without a production caller yet; tested"
    )
)]
pub fn truncate(
    m: &Metrics,
    text: &[u16],
    limit: i32,
    images: &Images,
) -> anyhow::Result<Vec<u16>> {
    truncate_with_provider(m, text, limit, images, images.provider())
}
pub fn truncate_with_provider(
    m: &Metrics,
    text: &[u16],
    limit: i32,
    images: &Images,
    provider: Option<&dyn IconProvider>,
) -> anyhow::Result<Vec<u16>> {
    if images.measure(m, text, true, provider)? <= limit {
        return Ok(text.to_vec());
    }
    let dots = [46, 46, 46];
    let limit = limit.wrapping_sub(m.width_utf16(&dots, None)?);
    let (mut open, mut previous, mut width, mut keep) = (None, None, 0i32, 0);
    for (i, &unit) in text.iter().enumerate() {
        let mut c = unit;
        if c == 60 {
            open = Some(i);
            continue;
        }
        if c == 62 {
            if let Some(start) = open.take() {
                let tag = String::from_utf16_lossy(&text[start + 1..i]);
                if let Some(ch) = entity(&tag) {
                    c = ch;
                } else {
                    let w = if let Some(v) = tag.strip_prefix("img=") {
                        v.parse::<i32>()
                            .ok()
                            .and_then(|id| images.images.as_ref()?.get(id as usize))
                            .map(|im| im.width)
                    } else if let Some(v) = tag.strip_prefix("sprite=") {
                        v.split(',')
                            .next()
                            .unwrap()
                            .parse::<i32>()
                            .ok()
                            .and_then(|id| provider?.width(id).ok())
                    } else {
                        None
                    };
                    if let Some(w) = w {
                        width = width.wrapping_add(w);
                        previous = None;
                        if width > limit {
                            return Ok([&text[..keep], &dots].concat());
                        }
                        keep = i + 1;
                    }
                    continue;
                }
            }
        }
        if open.is_none() {
            width = width
                .wrapping_add(m.advances[rs910_core::cp1252::cp1252_encode_unit(c) as usize] as i32)
                .wrapping_add(kern(m, previous, c)?);
            previous = Some(c);
            let with_dot = width.wrapping_add(kern(m, Some(c), 46)?);
            if with_dot > limit {
                return Ok([&text[..keep], &dots].concat());
            }
            keep = i + 1;
        }
    }
    Ok(text.to_vec())
}

/// FontMetrics.splitInit:402-555, including empty lines, hyphen breaks, dropped
/// spaces, and the reference's discarded sprite-icon advance in this method.
pub fn split(
    m: &Metrics,
    text: &[u16],
    limits: Option<&[i32]>,
    images: &Images,
    drop_space: bool,
    capacity: usize,
) -> anyhow::Result<(Vec<Vec<u16>>, usize)> {
    let icon = |id| Ok(images.icon_width(id));
    split_with_provider(
        m,
        text,
        limits,
        images,
        drop_space,
        capacity,
        images
            .icons
            .as_ref()
            .map(|_| &icon as &dyn Fn(i32) -> anyhow::Result<i32>),
    )
}

/// The resource lookup still occurs when the icon advance is discarded.
/// A provider exception leaves the preceding kerning character unchanged.
pub fn split_with_provider(
    m: &Metrics,
    text: &[u16],
    limits: Option<&[i32]>,
    images: &Images,
    drop_space: bool,
    capacity: usize,
    icons: Option<&dyn Fn(i32) -> anyhow::Result<i32>>,
) -> anyhow::Result<(Vec<Vec<u16>>, usize)> {
    let (mut width, mut start, mut breakpoint, mut break_width, mut drop) =
        (0i32, 0, None, 0i32, 0usize);
    let (mut open, mut previous) = (None, None);
    let mut lines = Vec::new();
    for (i, &unit) in text.iter().enumerate() {
        let mut c = rs910_core::cp1252::cp1252_encode_unit(unit) as i32;
        let mut advance = 0i32;
        let token_start;
        if c == 60 {
            open = Some(i);
            continue;
        }
        if let Some(tag_start) = open {
            if c != 62 {
                continue;
            }
            token_start = tag_start;
            let tag = String::from_utf16_lossy(&text[tag_start + 1..i]);
            open = None;
            if tag == "br" {
                anyhow::ensure!(lines.len() < capacity, "split output index");
                lines.push(text[start..i + 1].to_vec());
                if lines.len() >= capacity {
                    return Ok((lines, 0));
                }
                start = i + 1;
                width = 0;
                breakpoint = None;
                previous = None;
                continue;
            }
            if let Some(ch) = entity(&tag) {
                advance = m.advances[ch as usize] as i32 + kern(m, previous, ch)?;
                previous = Some(ch);
            } else if let Some(v) = tag.strip_prefix("img=") {
                if let Some(im) = v
                    .parse::<i32>()
                    .ok()
                    .and_then(|id| images.images.as_ref()?.get(id as usize))
                {
                    advance = im.width;
                    previous = None;
                }
            } else if let Some(icons) = icons.filter(|_| tag.starts_with("sprite=")) {
                if let Ok(id) = tag[7..].split(',').next().unwrap().parse::<i32>() {
                    if icons(id).is_ok() {
                        previous = None;
                    }
                }
                continue;
            }
            c = -1;
        } else {
            token_start = i;
            advance = m.advances[c as usize] as i32 + kern(m, previous, c as u16)?;
            previous = Some(c as u16);
        }
        if advance <= 0 {
            continue;
        }
        width = width.wrapping_add(advance);
        if let Some(limits) = limits {
            if c == 32 {
                breakpoint = Some(i);
                break_width = width;
                drop = drop_space as usize;
            }
            let limit = *limits
                .get(lines.len().min(limits.len().saturating_sub(1)))
                .ok_or_else(|| anyhow::anyhow!("split width index"))?;
            if width > limit {
                anyhow::ensure!(lines.len() < capacity, "split output index");
                if let Some(b) = breakpoint {
                    lines.push(text[start..b + 1 - drop].to_vec());
                    if lines.len() >= capacity {
                        return Ok((lines, 0));
                    }
                    start = b + 1;
                    breakpoint = None;
                    width = width.wrapping_sub(break_width);
                    previous = None;
                } else {
                    lines.push(text[start..token_start].to_vec());
                    if lines.len() >= capacity {
                        return Ok((lines, 0));
                    }
                    start = token_start;
                    breakpoint = None;
                    width = advance;
                    previous = None;
                }
            }
            if c == 45 {
                breakpoint = Some(i);
                break_width = width;
                drop = 0;
            }
        }
    }
    if text.len() > start {
        anyhow::ensure!(lines.len() < capacity, "split output index");
        lines.push(text[start..].to_vec());
    }
    let count = lines.len();
    Ok((lines, count))
}

#[derive(Clone, Copy)]
pub struct Paragraph {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub colour: i32,
    pub shadow: i32,
    pub halign: i32,
    pub valign: i32,
    pub line_height: i32,
    pub max_lines: i32,
    pub masked: bool,
}
/// Font.drawStringTaggable:79-145. All line/style state survives across lines
/// in one call; a <br> resets it through the same drawChars tag consumer.
#[cfg_attr(
    not(test),
    allow(
        dead_code,
        reason = "Font.drawStringTaggable port without a production caller yet; tested"
    )
)]
pub fn paragraph(
    m: &Metrics,
    text: &[u16],
    p: Paragraph,
    images: &Images,
    style: &mut Style,
    out: &mut Vec<Draw>,
) -> anyhow::Result<i32> {
    paragraph_to(m, Some(text), p, images, images.provider(), style, out)
}
pub fn paragraph_to(
    m: &Metrics,
    text: Option<&[u16]>,
    p: Paragraph,
    images: &Images,
    provider: Option<&dyn IconProvider>,
    style: &mut Style,
    out: &mut dyn Sink,
) -> anyhow::Result<i32> {
    let Some(text) = text else { return Ok(0) };
    *style = Style::new(p.colour, p.shadow);
    let mut line_height = if p.line_height == 0 {
        m.space_width
    } else {
        p.line_height
    };
    let widths = [p.width];
    let limits = if p.height < m.descent.wrapping_add(m.ascent).wrapping_add(line_height)
        && p.height < line_height.wrapping_add(line_height)
    {
        None
    } else {
        Some(&widths[..])
    };
    let width = |id| provider.unwrap().width(id);
    let (mut lines, mut count) = split_with_provider(
        m,
        text,
        limits,
        images,
        true,
        100,
        provider.map(|_| &width as &dyn Fn(i32) -> anyhow::Result<i32>),
    )?;
    let mut max = p.max_lines;
    if max == -1 {
        anyhow::ensure!(line_height != 0, "line height zero");
        max = p.height.wrapping_div(line_height).max(1);
    }
    if max > 0 && count >= max as usize {
        lines[max as usize - 1] =
            truncate_with_provider(m, &lines[max as usize - 1], p.width, images, provider)?;
        count = max as usize;
    }
    let mut valign = p.valign;
    if valign == 3 && count == 1 {
        valign = 1;
    }
    let count = count as i32;
    let span = count.wrapping_sub(1).wrapping_mul(line_height);
    let spare = p
        .height
        .wrapping_sub(m.ascent)
        .wrapping_sub(m.descent)
        .wrapping_sub(span);
    let mut baseline = match valign {
        0 => m.ascent.wrapping_add(p.y),
        1 => (spare / 2).wrapping_add(m.ascent).wrapping_add(p.y),
        2 => {
            p.y.wrapping_add(p.height)
                .wrapping_sub(m.descent)
                .wrapping_sub(span)
        }
        _ => {
            anyhow::ensure!(count != -1, "vertical spacing divisor");
            let gap = spare.wrapping_div(count.wrapping_add(1)).max(0);
            line_height = line_height.wrapping_add(gap);
            m.ascent.wrapping_add(p.y).wrapping_add(gap)
        }
    };
    for (n, t) in lines.iter().take(count as usize).enumerate() {
        let x = match p.halign {
            0 => p.x,
            1 => {
                p.x.wrapping_add(p.width.wrapping_sub(images.measure(m, t, false, provider)?) / 2)
            }
            2 => {
                p.x.wrapping_add(p.width)
                    .wrapping_sub(images.measure(m, t, false, provider)?)
            }
            _ => {
                if n + 1 != count as usize {
                    style.justify_with_provider(m, t, p.width, images, provider)?;
                }
                p.x
            }
        };
        line_to(
            m,
            LineAt {
                text: t,
                x,
                baseline,
            },
            style,
            TextResources { images, provider },
            p.masked,
            out,
        )?;
        if p.halign != 0 && p.halign != 1 && p.halign != 2 && n + 1 != count as usize {
            style.spacing = 0;
        }
        baseline = baseline.wrapping_add(line_height);
    }
    Ok(count)
}

/// The 48-bit LCG random generator, used by the jittered text form.
/// The owner retains the final state.
#[derive(Default, Clone, Copy)]
pub struct Random {
    pub seed: u64,
}
impl Random {
    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed as u64 ^ 0x5deece66d) & ((1u64 << 48) - 1);
    }
    pub fn next_int(&mut self) -> i32 {
        self.seed = self.seed.wrapping_mul(0x5deece66d).wrapping_add(11) & ((1u64 << 48) - 1);
        (self.seed >> 16) as i32
    }
}
/// The shared random owner, its seed and the optional hover bounds the
/// jittered ("antimacro") text form takes.
pub struct Antimacro<'a> {
    pub random: &'a mut Random,
    pub seed: i32,
    pub bounds: Option<&'a mut [i32]>,
}

/// The jittered text form. The caller supplies the shared random owner/seed
/// and optional hover bounds; no text leaves all three untouched.
pub fn antimacro_to(
    m: &Metrics,
    text: Option<&[u16]>,
    p: Paragraph,
    resources: TextResources<'_>,
    style: &mut Style,
    jitter_state: Antimacro<'_>,
    out: &mut dyn Sink,
) -> anyhow::Result<i32> {
    let TextResources { images, provider } = resources;
    let Antimacro {
        random,
        seed,
        mut bounds,
    } = jitter_state;
    let Some(text) = text else { return Ok(0) };
    random.set_seed(seed as i64);
    let alpha = (random.next_int() & 31) + 192;
    *style = Style::new(
        alpha << 24 | p.colour & 0xffffff,
        if p.shadow == -1 {
            0
        } else {
            alpha << 24 | p.shadow & 0xffffff
        },
    );
    let mut offsets = Vec::with_capacity(text.len());
    let mut jitter = 0i32;
    for _ in text {
        offsets.push(jitter);
        if random.next_int() & 3 == 0 {
            jitter = jitter.wrapping_add(1);
        }
    }
    let mut x = p.x;
    let mut baseline = m.ascent.wrapping_add(p.y);
    let mut width = -1;
    if p.valign == 1 {
        baseline =
            baseline.wrapping_add(p.height.wrapping_sub(m.ascent).wrapping_sub(m.descent) / 2);
    } else if p.valign == 2 {
        baseline = p.y.wrapping_add(p.height).wrapping_sub(m.descent);
    }
    if p.halign == 1 || p.halign == 2 {
        width = images
            .measure(m, text, false, provider)?
            .wrapping_add(jitter);
        x = if p.halign == 1 {
            p.width.wrapping_sub(width) / 2
        } else {
            p.width.wrapping_sub(width)
        }
        .wrapping_add(p.x);
    }
    alpha_line_to(
        m,
        LineAt { text, x, baseline },
        style,
        resources,
        Offsets {
            x: Some(&offsets),
            y: None,
        },
        out,
    )?;
    if let Some(bounds) = bounds.as_mut() {
        if width == -1 {
            width = images
                .measure(m, text, false, provider)?
                .wrapping_add(jitter);
        }
        for (i, value) in [
            x,
            baseline.wrapping_sub(m.ascent),
            width,
            m.descent.wrapping_add(m.ascent),
        ]
        .into_iter()
        .enumerate()
        {
            *bounds
                .get_mut(i)
                .ok_or_else(|| anyhow::anyhow!("hover bounds index"))? = value;
        }
    }
    Ok(jitter)
}
