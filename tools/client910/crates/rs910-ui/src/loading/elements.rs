//! Loading-screen element readiness, animation and painting.

use crate::{
    font_layout::{self, Draw, Images, Paragraph, Sink, Style},
    ui_fonts::Font,
    ui_paint::Painter,
    ui_sprites::Sprite,
};

use anyhow::{Context as _, Result};

use rs910_core::fault::Fault;

use std::rc::Rc;

use super::{Align, ElementConfig, NewsManager, ProgressConfig, Resources, ScreenLayout, Snapshot};

// ---------------------------------------------------------------------------
// Elements
// ---------------------------------------------------------------------------

/// What an element's draw reads from the presenter and the window.
pub struct DrawContext<'a> {
    pub painter: &'a mut Painter,
    /// Canvas width and height.
    pub canvas: [i32; 2],
    /// Frame width and height.
    pub frame: [i32; 2],
    /// The monotonic clock.
    pub now: i64,
    pub snapshot: &'a Snapshot,
    /// The presenter's draw count.
    pub draws: i32,
    /// The b12 full font (news entries).
    pub b12: Option<Rc<Font>>,
}

impl DrawContext<'_> {
    pub(super) fn x(&self, align: Align, size: i32) -> i32 {
        align.compute(size, self.canvas[0], self.frame[0])
    }
    pub(super) fn y(&self, align: Align, size: i32) -> i32 {
        align.compute(size, self.canvas[1], self.frame[1])
    }
}

/// A loading-screen element with the resources `create` made and its
/// draw-time state.
pub struct Element {
    pub config: ElementConfig,
    pub(super) font: Option<Rc<Font>>,
    pub(super) sprites: Vec<Rc<Sprite>>,
    /// Rotation of a rotating sprite.
    pub(super) angle: i32,
    /// The percent last shown and when it was first shown (bars and the
    /// status text).
    pub(super) shown_percent: i32,
    pub(super) since: i64,
    pub(super) news: Option<Rc<std::cell::RefCell<NewsManager>>>,
}

impl Element {
    /// Builds the element for a config; news elements share the fetcher.
    pub(super) fn new(config: ElementConfig, resources: &mut Resources) -> Self {
        let news = matches!(config, ElementConfig::News(_)).then(|| resources.news());
        // The status text's clock starts unset (-1).
        let since = if matches!(config, ElementConfig::Status(_)) {
            -1
        } else {
            0
        };
        Self {
            config,
            font: None,
            sprites: Vec::new(),
            angle: 0,
            shown_percent: 0,
            since,
            news,
        }
    }

    pub(super) fn sprite_ids(&self) -> Vec<i32> {
        match &self.config {
            ElementConfig::Sprite(s) | ElementConfig::RotatingSprite { sprite: s, .. } => {
                vec![s.sprite]
            }
            ElementConfig::Background { sprite } => vec![*sprite],
            ElementConfig::SpriteBar { sprite, .. } => vec![*sprite],
            ElementConfig::SlicedBar { sprites, .. }
            | ElementConfig::ScrollingBar { sprites, .. } => sprites.ids().to_vec(),
            _ => Vec::new(),
        }
    }

    pub(super) fn font_id(&self) -> Option<i32> {
        match &self.config {
            ElementConfig::ColourBar { bar, .. }
            | ElementConfig::SpriteBar { bar, .. }
            | ElementConfig::SlicedBar { bar, .. }
            | ElementConfig::ScrollingBar { bar, .. } => Some(bar.font),
            ElementConfig::Text(t) => Some(t.font),
            ElementConfig::Status(s) => Some(s.font),
            _ => None,
        }
    }

    /// Whether the element's archives are loaded.
    pub fn is_ready(&mut self, resources: &mut Resources) -> Result<bool> {
        if let Some(news) = &self.news {
            return Ok(news.borrow_mut().ready());
        }
        if let Some(font) = self.font_id() {
            if !resources.font_ready(font)? {
                return Ok(false);
            }
        }
        for id in self.sprite_ids() {
            if !resources.sprite_ready(id)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Creates the font and sprites once they are ready.
    pub fn create(&mut self, resources: &mut Resources) -> Result<()> {
        if let Some(font) = self.font_id() {
            self.font = Some(resources.font(font)?);
        }
        self.sprites = self
            .sprite_ids()
            .into_iter()
            .map(|id| resources.sprite(id))
            .collect::<Result<_>>()?;
        Ok(())
    }

    /// Draws the element; `full` is a full redraw.
    pub fn draw(&mut self, full: bool, ctx: &mut DrawContext<'_>) -> Result<()> {
        let config = self.config.clone();
        match &config {
            // Clear: only on a full redraw.
            ElementConfig::Clear { colour } => {
                if full {
                    ctx.painter
                        .fill([0, 0, ctx.canvas[0], ctx.canvas[1]], *colour)?;
                }
            }
            // Sprite at its aligned position.
            ElementConfig::Sprite(s) => {
                if full {
                    let sprite = self.sprites.first().context("sprite not created")?;
                    let [w, h] = sprite.full_size();
                    let x = ctx.x(s.halign, w) + s.offset[0];
                    let y = ctx.y(s.valign, h) + s.offset[1];
                    ctx.painter.native(sprite, [x, y], -1);
                }
            }
            // Rotating sprite: every frame, about the centre.
            ElementConfig::RotatingSprite { sprite: s, speed } => {
                let sprite = self.sprites.first().context("sprite not created")?.clone();
                let [w, h] = sprite.full_size();
                let x = ctx.x(s.halign, w) + s.offset[0];
                let y = ctx.y(s.valign, h) + s.offset[1];
                // Rotates about the sprite centre at scale 4096.
                ctx.painter.rotated_image(crate::ui_paint::RotatedImage {
                    image: crate::ui_paint::Image::Sprite(sprite),
                    size: [w, h],
                    origin: [(x + w / 2) as f32, (y + h / 2) as f32],
                    pivot: [w as f32 / 2.0, h as f32 / 2.0],
                    scale: 4096,
                    angle: self.angle,
                    colour: -1,
                    mask: None,
                });
                self.angle = self.angle.wrapping_add(*speed);
            }
            // Background: aspect-fit over max(canvas, frame).
            ElementConfig::Background { .. } => {
                if full {
                    let sprite = self.sprites.first().context("sprite not created")?;
                    let width = ctx.canvas[0].max(ctx.frame[0]);
                    let height = ctx.canvas[1].max(ctx.frame[1]);
                    let [sw, sh] = sprite.full_size();
                    anyhow::ensure!(
                        sw != 0 && sh != 0,
                        Fault::DivisionByZero.message("background sprite size")
                    );
                    let mut x = 0;
                    let mut w = width;
                    let mut h = width * sh / sw;
                    let mut y = (height - h) / 2;
                    if h > height {
                        y = 0;
                        h = height;
                        w = height * sw / sh;
                        x = (width - w) / 2;
                    }
                    ctx.painter.scaled(sprite, [x, y, w, h], -1)?;
                }
            }
            // Paragraph text at its aligned position.
            ElementConfig::Text(t) => {
                if full {
                    let font = self.font.clone().context("font not created")?;
                    let x = ctx.x(t.halign, t.size[0]) + t.offset[0];
                    let y = ctx.y(t.valign, t.size[1]) + t.offset[1];
                    draw_string_taggable(
                        ctx.painter,
                        &font,
                        &t.text,
                        [x, y, t.size[0], t.size[1]],
                        [t.colour, t.shadow],
                        [t.text_halign, t.text_valign],
                        t.line_height,
                    )?;
                }
            }
            // Status text: after 10 s in one state the stage id is appended.
            ElementConfig::Status(s) => {
                let font = self.font.clone().context("font not created")?;
                let x = ctx.x(s.halign, 0) + s.offset[0];
                let y = ctx.y(s.valign, 0) + s.offset[1];
                let snapshot = ctx.snapshot;
                let mut text = match s.kind {
                    0 => snapshot.text.clone(),
                    1 => format!("{}%", snapshot.percent),
                    2 => snapshot.text_percent.clone(),
                    _ => String::new(),
                };
                let percent = snapshot.percent;
                if self.since < 0 || percent == 0 || self.shown_percent != percent {
                    self.since = ctx.now;
                    self.shown_percent = percent;
                }
                if s.kind != 1 && ctx.now - self.since > 10_000 {
                    text = format!("{text} ({})", snapshot.stage_id());
                }
                draw_string_center(ctx.painter, &font, &text, [x, y], s.colour, -1)?;
            }
            // Progress bars: frame, fill, then the percent text.
            ElementConfig::ColourBar { bar, .. }
            | ElementConfig::SpriteBar { bar, .. }
            | ElementConfig::SlicedBar { bar, .. }
            | ElementConfig::ScrollingBar { bar, .. } => {
                let x = ctx.x(bar.halign, bar.size[0]) + bar.offset[0];
                let y = ctx.y(bar.valign, bar.size[1]) + bar.offset[1];
                self.draw_bar_back(full, x, y, bar, ctx)?;
                self.draw_bar_fill(x, y, bar, ctx)?;
                let mut text = ctx.snapshot.text_percent.clone();
                if ctx.now - self.since > 10_000 {
                    text = format!("{text} ({})", ctx.snapshot.stage_id());
                }
                let font = self.font.clone().context("font not created")?;
                draw_string_center(
                    ctx.painter,
                    &font,
                    &text,
                    [
                        bar.size[0] / 2 + x,
                        bar.text_offset + bar.size[1] / 2 + y + 4,
                    ],
                    bar.colour,
                    -1,
                )?;
            }
            // News entry: title, rule, then two paragraphs.
            ElementConfig::News(n) => {
                let Some(entry) = self
                    .news
                    .as_ref()
                    .and_then(|news| news.borrow().entry(n.entry))
                else {
                    return Ok(());
                };
                let font = ctx
                    .b12
                    .clone()
                    .with_context(|| Fault::MissingValue.message("b12 full font"))?;
                let x = ctx.x(n.halign, n.size[0]) + n.offset[0];
                let y = ctx.y(n.valign, n.size[1]) + n.offset[1];
                if n.draw_border {
                    ctx.painter.outline([x, y, n.size[0], n.size[1]], n.border);
                }
                let para = |painter: &mut Painter, text: &str, y: i32| {
                    // Paragraph inside the box with a 5 px margin.
                    draw_string_taggable(
                        painter,
                        &font,
                        text,
                        [x + 5, y + 5, n.size[0] - 10, n.size[1] - 10],
                        [n.colour, n.shadow],
                        [0, 0],
                        0,
                    )
                };
                let line = y + para(ctx.painter, &entry[0], y)? * 12 + 8;
                if n.draw_border {
                    ctx.painter
                        .line([x, line], [n.size[0] + x - 1, line], n.border, 1);
                }
                let body = line + 1;
                let second = body + para(ctx.painter, &entry[1], body)? * 12 + 5;
                para(ctx.painter, &entry[2], second)?;
            }
        }
        Ok(())
    }

    /// The bar frame.
    pub(super) fn draw_bar_back(
        &self,
        full: bool,
        x: i32,
        y: i32,
        bar: &ProgressConfig,
        ctx: &mut DrawContext<'_>,
    ) -> Result<()> {
        let [w, h] = bar.size;
        match &self.config {
            // Colour and sprite bars: a double outline.
            ElementConfig::ColourBar { outline, .. } | ElementConfig::SpriteBar { outline, .. } => {
                ctx.painter.outline([x - 2, y, w + 4, h + 2], *outline);
                ctx.painter.outline([x - 1, y + 1, w + 2, h], 0);
            }
            // Six-sprite bars: caps and tiled top/bottom edges (full redraw only).
            _ => {
                if !full {
                    return Ok(());
                }
                let [_, _, left, right, top, bottom] = self.bar_sprites()?;
                let saved = ctx.painter.sprite.clip;
                ctx.painter.reset_bounds([x, y, w + x, h + y]);
                let [lw, lh] = left.full_size();
                let [rw, rh] = right.full_size();
                ctx.painter.native(&left, [x, (h - lh) / 2 + y], -1);
                ctx.painter
                    .native(&right, [w + x - rw, (h - rh) / 2 + y], -1);
                ctx.painter
                    .reset_bounds([x, y, w + x, y + top.full_size()[1]]);
                ctx.painter.tiled(&top, [x + lw, y, w - lw - rw, h], 1)?;
                ctx.painter.quad_count();
                let bh = bottom.full_size()[1];
                ctx.painter.reset_bounds([x, h + y - bh, w + x, h + y]);
                ctx.painter
                    .tiled(&bottom, [x + lw, h + y - bh, w - lw - rw, h], 1)?;
                ctx.painter.quad_count();
                ctx.painter.reset_bounds(saved);
            }
        }
        Ok(())
    }

    pub(super) fn bar_sprites(&self) -> Result<[Rc<Sprite>; 6]> {
        let s = &self.sprites;
        anyhow::ensure!(s.len() == 6, "bar sprites not created");
        Ok([
            s[0].clone(),
            s[1].clone(),
            s[2].clone(),
            s[3].clone(),
            s[4].clone(),
            s[5].clone(),
        ])
    }

    /// The progress fill.
    pub(super) fn draw_bar_fill(
        &mut self,
        x: i32,
        y: i32,
        bar: &ProgressConfig,
        ctx: &mut DrawContext<'_>,
    ) -> Result<()> {
        let [w, h] = bar.size;
        let progress = self.interpolated_percent(ctx);
        match self.config.clone() {
            // Colour bar: fill, then the empty remainder.
            ElementConfig::ColourBar { fill, .. } => {
                let filled = progress * w / 10_000;
                ctx.painter.fill([x, y + 2, filled, h - 2], fill)?;
                ctx.painter
                    .fill([x + filled, y + 2, w - filled, h - 2], 0)?;
            }
            // Sprite bar: the tiled sprite clipped to the filled width.
            ElementConfig::SpriteBar { .. } => {
                let filled = progress * w / 10_000;
                let sprite = self.sprites.first().context("sprite not created")?.clone();
                let saved = ctx.painter.sprite.clip;
                ctx.painter.reset_bounds([x, y + 2, x + filled, h + y]);
                ctx.painter.tiled(&sprite, [x, y + 2, w, h], 1)?;
                ctx.painter.quad_count();
                ctx.painter.reset_bounds(saved);
            }
            // Six-sprite bars (plain and scrolling).
            config => {
                let [fill, empty, left, right, top, bottom] = self.bar_sprites()?;
                let x0 = x + left.full_size()[0];
                let x1 = w + x - right.full_size()[0];
                let y0 = y + top.full_size()[1];
                let y1 = h + y - bottom.full_size()[1];
                let dw = x1 - x0;
                let dh = y1 - y0;
                let filled = progress * dw / 10_000;
                let saved = ctx.painter.sprite.clip;
                ctx.painter.reset_bounds([x0, y0, x0 + filled, y1]);
                if let ElementConfig::ScrollingBar { speed, .. } = config {
                    // The scrolling bar shifts the fill by draw count.
                    let fw = fill.full_size()[0];
                    anyhow::ensure!(fw != 0, Fault::DivisionByZero.message("fill sprite width"));
                    let shift = speed.wrapping_mul(ctx.draws) / 10 % fw;
                    ctx.painter
                        .tiled(&fill, [x0 - fw + shift, y0, dw + fw - shift, dh], 1)?;
                } else {
                    // The plain bar tiles the fill unshifted.
                    ctx.painter.tiled(&fill, [x0, y0, dw, dh], 1)?;
                }
                ctx.painter.quad_count();
                ctx.painter.reset_bounds([x0 + filled, y0, x1, y1]);
                ctx.painter.tiled(&empty, [x0, y0, dw, dh], 1)?;
                ctx.painter.quad_count();
                ctx.painter.reset_bounds(saved);
            }
        }
        Ok(())
    }

    /// The shown percent x100, eased toward [`Snapshot::next_percent`] over
    /// the time the previous step took.
    pub(super) fn interpolated_percent(&mut self, ctx: &DrawContext<'_>) -> i32 {
        let current = ctx.snapshot.percent;
        let mut shown = current * 100;
        if self.shown_percent == current && current != 0 {
            let target = ctx.snapshot.next_percent();
            if target > current {
                let elapsed = self.since - ctx.snapshot.start_time;
                if elapsed > 0 {
                    let span = elapsed * 10_000 / i64::from(current) * i64::from(target - current);
                    let now = (ctx.now - self.since) * 10_000;
                    shown = if now < span {
                        (i64::from(target - current) * now * 100 / span + i64::from(current * 100))
                            as i32
                    } else {
                        target * 100
                    };
                }
            }
        } else {
            self.shown_percent = current;
            self.since = ctx.now;
        }
        shown
    }
}

/// Paint `font_layout` terminals through the shared painter. The 910 client
/// passes no sprite arrays to these draws, so `<img>` tags draw nothing.
pub(super) struct PaintSink<'a> {
    painter: &'a mut Painter,
    font: &'a Rc<Font>,
}

impl Sink for PaintSink<'_> {
    fn emit(&mut self, draw: Draw) -> Result<()> {
        match draw {
            Draw::Glyph { .. } => self.painter.glyph(self.font, &draw),
            Draw::Line {
                x,
                y,
                width,
                colour,
            } => self
                .painter
                .line([x, y], [x.wrapping_add(width), y], colour, 1),
            Draw::Image { .. } => {}
        }
        Ok(())
    }
}

pub(super) fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Draws `text` horizontally centred on `x`.
pub fn draw_string_center(
    painter: &mut Painter,
    font: &Rc<Font>,
    text: &str,
    [x, y]: [i32; 2],
    colour: i32,
    shadow: i32,
) -> Result<()> {
    let text = utf16(text);
    let width = font.metrics.width_utf16(&text, None)?;
    draw_string(painter, font, &text, [x - width / 2, y], colour, shadow)
}

/// Draws `text` with its left end at `x`.
pub fn draw_string(
    painter: &mut Painter,
    font: &Rc<Font>,
    text: &[u16],
    [x, y]: [i32; 2],
    colour: i32,
    shadow: i32,
) -> Result<()> {
    let mut style = Style::new(colour, shadow);
    font_layout::line_to(
        &font.metrics,
        font_layout::LineAt {
            text,
            x,
            baseline: y,
        },
        &mut style,
        font_layout::TextResources {
            images: &Images::default(),
            provider: None,
        },
        false,
        &mut PaintSink { painter, font },
    )
}

/// Draws tagged paragraph text in a box, without sprites or line limit;
/// returns the line count.
pub fn draw_string_taggable(
    painter: &mut Painter,
    font: &Rc<Font>,
    text: &str,
    [x, y, width, height]: [i32; 4],
    [colour, shadow]: [i32; 2],
    [halign, valign]: [i32; 2],
    line_height: i32,
) -> Result<i32> {
    let text = utf16(text);
    let mut style = Style::default();
    font_layout::paragraph_to(
        &font.metrics,
        Some(&text),
        Paragraph {
            x,
            y,
            width,
            height,
            colour,
            shadow,
            halign,
            valign,
            line_height,
            max_lines: 0,
            masked: false,
        },
        &Images::default(),
        None,
        &mut style,
        &mut PaintSink { painter, font },
    )
}

// ---------------------------------------------------------------------------
// Cache loading screens
// ---------------------------------------------------------------------------

/// A cache-defined loading screen: its elements, minimum display time and
/// cross-fade time.
pub struct CacheScreen {
    pub id: i32,
    pub elements: Vec<Option<Element>>,
    pub min_time: i32,
    pub fade: i32,
}

impl CacheScreen {
    /// Builds the screen's elements from its layout.
    pub fn new(
        id: i32,
        layout: ScreenLayout,
        min_time: i32,
        fade: i32,
        resources: &mut Resources,
    ) -> Self {
        Self {
            id,
            elements: layout
                .elements
                .into_iter()
                .map(|config| config.map(|c| Element::new(c, resources)))
                .collect(),
            min_time,
            fade,
        }
    }

    /// Creates every element.
    pub fn create(&mut self, resources: &mut Resources) -> Result<()> {
        for element in self.elements.iter_mut().flatten() {
            element.create(resources)?;
        }
        Ok(())
    }

    /// Draws every element in full.
    pub fn draw(&mut self, ctx: &mut DrawContext<'_>) -> Result<()> {
        for element in self.elements.iter_mut().flatten() {
            element.draw(true, ctx)?;
        }
        Ok(())
    }

    /// Percent of empty-or-ready elements.
    pub fn ready_percent(&mut self, resources: &mut Resources) -> Result<i32> {
        let total = self.elements.len() as i32;
        anyhow::ensure!(
            total != 0,
            Fault::DivisionByZero.message("screen has no elements")
        );
        let mut ready = 0;
        for element in &mut self.elements {
            match element {
                None => ready += 1,
                Some(e) => {
                    if e.is_ready(resources)? {
                        ready += 1;
                    }
                }
            }
        }
        Ok(ready * 100 / total)
    }

    /// Whether the minimum display time has passed since `since`.
    #[must_use]
    pub fn shown_long_enough(&self, since: i64, now: i64) -> bool {
        now >= i64::from(self.min_time) + since
    }
}
