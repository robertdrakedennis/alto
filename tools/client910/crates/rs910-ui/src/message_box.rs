//! The boxed status text drawn over the retained frame in the reconnect (14),
//! transfer (19) and rebuild states, styled by the `setup_messagebox` script
//! command.
use crate::{
    loading::{draw_string_taggable, Align},
    sprite_data::Data,
    ui_fonts::{Archive, Fonts},
    ui_paint::Painter,
    ui_sprites::Sprite,
};
use anyhow::{Context as _, Result};
use rs910_core::fault::Fault;
use std::rc::Rc;

/// The box's style, set by the `setup_messagebox` script command (the
/// `host_builtins` owner stores it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageBox {
    pub setup: bool,
    /// Horizontal and vertical alignment indices (0..3).
    pub halign: usize,
    pub valign: usize,
    pub box_xy: [i32; 2],
    pub min_size: [i32; 2],
    pub border_corner: i32,
    pub border_line: i32,
    pub background: i32,
    pub colour: i32,
    pub font: i32,
}

/// The line-break tag in message-box text.
pub const BR: &str = "<br>";

/// The first sprite frame of sprite archive entry `id`, if it is cached.
fn sprite_data(fonts: &Fonts, id: i32) -> Result<Option<Data>> {
    let Some(bytes) = fonts.source_fetch(Archive::Sprites, id)? else {
        return Ok(None);
    };
    Ok(Data::decode(&bytes)?.into_iter().next())
}

/// Requests every configured resource; true once the sprites and font
/// metrics archives all hold them.
fn download(fonts: &Fonts, config: &MessageBox) -> Result<bool> {
    let mut ready = true;
    for id in [config.border_corner, config.border_line, config.background] {
        if !fonts.source_load(Archive::Sprites, id)? {
            ready = false;
        }
    }
    if !fonts.source_load(Archive::Metrics, config.font)? {
        ready = false;
    }
    if !fonts.source_load(Archive::Sprites, config.font)? {
        ready = false;
    }
    Ok(ready)
}

/// The widest line and the line count of `text` wrapped to 250 pixels.
fn paragraph(metrics: &crate::font_metrics::Metrics, text: &[u16]) -> Result<(i32, i32)> {
    let (lines, count) = crate::font_layout::split(
        metrics,
        text,
        Some(&[250]),
        &crate::font_layout::Images::default(),
        true,
        100,
    )?;
    let mut width = 0;
    for line in &lines[..count] {
        width = width.max(metrics.width_utf16(line, None)?);
    }
    Ok((width, count as i32))
}

/// Draws the message box for `text`. `canvas` is the canvas size and `frame`
/// the game frame size. Returns false, drawing nothing, when the configured
/// sprites are not yet downloaded.
pub fn draw(
    painter: &mut Painter,
    fonts: &Fonts,
    config: &MessageBox,
    text: &str,
    canvas: [i32; 2],
    frame: [i32; 2],
) -> Result<bool> {
    let text16: Vec<u16> = text.encode_utf16().collect();
    if config.setup {
        if !download(fonts, config)? {
            return Ok(false);
        }
        // The configured font: metrics plus their glyph sprites.
        let font = fonts
            .get_font(config.font, true, true)?
            .with_context(|| Fault::MissingValue.message("message box font"))?;
        let m = &font.metrics;
        let (width, lines) = paragraph(m, &text16)?;
        // Text height: descent plus ascent plus one line pitch per extra line.
        let height = m.descent + m.ascent + (lines - 1) * m.space_width;
        let corner = sprite_data(fonts, config.border_corner)?
            .with_context(|| Fault::MissingValue.message("border corner sprite"))?;
        let mut line = sprite_data(fonts, config.border_line)?
            .with_context(|| Fault::MissingValue.message("border line sprite"))?;
        let background = sprite_data(fonts, config.background)?
            .with_context(|| Fault::MissingValue.message("background sprite"))?;
        let border = line.width;
        let pad = border + 4;
        let w = (pad * 2 + width).max(config.min_size[0]);
        let h = (pad * 2 + height).max(config.min_size[1]);
        let halign = Align::from_index(config.halign)
            .with_context(|| Fault::MissingValue.message("horizontal alignment"))?;
        let valign = Align::from_index(config.valign)
            .with_context(|| Fault::MissingValue.message("vertical alignment"))?;
        let x = halign.compute(w, canvas[0], frame[0]) + config.box_xy[0];
        let y = valign.compute(h, canvas[1], frame[1]) + config.box_xy[1];
        let [cw, ch] = [corner.width, corner.height];
        let sprite = |data: &Data| -> Result<Rc<Sprite>> { Ok(Rc::new(Sprite::new(data)?)) };
        // The background tiles inside the corners and is never the paletted
        // software sprite.
        let mut background = Sprite::new(&background)?;
        background.paletted = None;
        painter.tiled(
            &Rc::new(background),
            [x + cw, y + ch, w - cw * 2, h - ch * 2],
            0,
        )?;
        painter.quad_count();
        let mut corner = corner;
        painter.native(&sprite(&corner)?, [x, y], -1);
        corner.flip(false);
        painter.native(&sprite(&corner)?, [w + x - border, y], -1);
        corner.flip(true);
        painter.native(&sprite(&corner)?, [w + x - border, h + y - border], -1);
        corner.flip(false);
        painter.native(&sprite(&corner)?, [x, h + y - border], -1);
        let edges = [
            [x, y + ch, border, h - ch * 2],
            [x + cw, y, w - cw * 2, border],
            [w + x - border, y + ch, border, h - ch * 2],
            [x + cw, h + y - border, w - cw * 2, border],
        ];
        for (i, rect) in edges.into_iter().enumerate() {
            if i > 0 {
                line.rotate();
            }
            painter.tiled(&sprite(&line)?, rect, 1)?;
            painter.quad_count();
        }
        draw_string_taggable(
            painter,
            &font,
            text,
            [pad + x, pad + y, w - pad * 2, h - pad * 2],
            [config.colour | 0xff00_0000_u32 as i32, -1],
            [1, 1],
            0,
        )?;
    } else {
        // The default 12-point font (second entry of the default font ids).
        let id = fonts
            .ids
            .as_ref()
            .and_then(|ids| ids.get(1).copied())
            .with_context(|| Fault::MissingValue.message("default 12-point font id"))?;
        let font = fonts
            .get_font(id, true, true)?
            .with_context(|| Fault::MissingValue.message("default 12-point font"))?;
        let (width, lines) = paragraph(&font.metrics, &text16)?;
        let height = lines * 13;
        let pad = 4;
        let x = pad + 6;
        let y = pad + 6;
        let rect = [x - pad, y - pad, width + pad + pad, height + pad + pad];
        painter.fill(rect, 0xff00_0000_u32 as i32)?;
        painter.outline(rect, -1);
        draw_string_taggable(
            painter,
            &font,
            text,
            [x, y, width, height],
            [-1, -1],
            [1, 1],
            0,
        )?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fonts() -> Fonts {
        Fonts::from_pack(
            crate::test_support::require_pack("client.sprites.js5"),
            Some(0),
        )
        .unwrap()
    }

    /// The unconfigured branch: a black box outlined in white at
    /// (6, 6), sized to the p12 paragraph, with the text centred inside.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn unconfigured_message_box_is_the_p12_black_box() {
        let fonts = fonts();
        let mut painter = Painter::new([800, 600]);
        let text = format!(
            "{}{BR}{}",
            crate::loading::Text::ConnectionLost.display(crate::ui_text_compare::Language::En),
            crate::loading::Text::AttemptToReestablish
                .display(crate::ui_text_compare::Language::En)
        );
        assert!(draw(
            &mut painter,
            &fonts,
            &MessageBox::default(),
            &text,
            [800, 600],
            [765, 553]
        )
        .unwrap());
        let plan = painter.finish();
        // fill + 4 outline lines + the glyphs of both lines.
        let glyphs = text.chars().filter(|c| !c.is_whitespace()).count() - "<br>".len();
        assert!(
            plan.quads.len() >= 5 + glyphs / 2,
            "{} quads",
            plan.quads.len()
        );
        let fill = &plan.quads[0];
        let xs: Vec<f32> = fill.vertices.iter().map(|v| v.position[0]).collect();
        // Left edge at x = 6 on an 800-wide canvas.
        let left = xs.iter().copied().fold(f32::MAX, f32::min);
        assert!((left - (6.0 / 800.0 * 2.0 - 1.0)).abs() < 1e-4, "{left}");
    }

    /// The configured branch waits for its sprites: a missing
    /// border sprite draws nothing.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn configured_message_box_waits_for_sprites() {
        let fonts = fonts();
        let mut painter = Painter::new([800, 600]);
        let config = MessageBox {
            setup: true,
            border_corner: i32::MAX,
            border_line: i32::MAX,
            background: i32::MAX,
            font: i32::MAX,
            ..MessageBox::default()
        };
        assert!(!draw(&mut painter, &fonts, &config, "x", [800, 600], [765, 553]).unwrap());
        assert_eq!(painter.finish().quads.len(), 0);
    }
}
