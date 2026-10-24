//! The progress bar drawn before cache loading screens are available.

use crate::{ui_fonts::Font, ui_paint::Painter};

use anyhow::Result;

use std::rc::Rc;

use super::{draw_string, utf16};

// ---------------------------------------------------------------------------
// Pre-loading bar
// ---------------------------------------------------------------------------

/// Loading bar fill, outline and text colours, indexed by applet
/// parameter 34.
pub(super) const LOADING_BAR_COLOURS: [[i32; 3]; 4] = [
    [9_179_409, 9_179_409, 16_777_215],
    [3_289_650, 16_777_215, 16_777_215],
    [3_289_650, 16_726_277, 16_741_381],
    [3_289_650, 16_726_277, 16_741_381],
];

/// The plain 304x34 outlined progress bar shown before the cache screens are
/// ready (the applet viewer launch has no bar configuration).
///
/// `loading_text` is the "Loading - please wait." text the client sets before
/// the first loading update; it is drawn above the bar.
///
/// Every placement is fixed arithmetic, independent of the font: the bar text
/// starts at `x + (304 - length * 6) / 2` on baseline `y + 22`, the loading
/// text at `canvas_width / 2 - length * 6 / 2` on baseline
/// `canvas_height / 2 - 26`, both by UTF-16 length. Only the glyphs (13 pt
/// bold Helvetica in the 910 client) come from the platform's font presenter:
/// this port bundles no font file and draws them with the closest cache font,
/// the graphics-defaults `b12_full` bold face from the loading sprites, once
/// stage 1 has named it.
pub fn draw_pre_loading(
    painter: &mut Painter,
    canvas: [i32; 2],
    percent: i32,
    text: &str,
    loading_text: Option<&str>,
    colour_index: usize,
    font: Option<&Rc<Font>>,
) -> Result<()> {
    let [fill, outline, text_colour] = LOADING_BAR_COLOURS
        .get(colour_index)
        .copied()
        .unwrap_or(LOADING_BAR_COLOURS[0]);
    let opaque = |rgb: i32| rgb | 0xff00_0000_u32 as i32;
    painter.fill([0, 0, canvas[0], canvas[1]], opaque(0))?;
    let x = canvas[0] / 2 - 152;
    let y = canvas[1] / 2 - 18;
    // An outline rectangle (x, y, w, h) covers w + 1 by h + 1 pixels.
    painter.outline([x, y, 304, 34], opaque(outline));
    painter.fill([x + 2, y + 2, percent * 3, 30], opaque(fill))?;
    painter.outline([x + 1, y + 1, 302, 32], opaque(0));
    painter.fill(
        [percent * 3 + x + 2, y + 2, 300 - percent * 3, 30],
        opaque(0),
    )?;
    if let Some(font) = font {
        let len = text.encode_utf16().count() as i32;
        draw_string(
            painter,
            font,
            &utf16(text),
            [x + (304 - len * 6) / 2, y + 22],
            opaque(text_colour),
            -1,
        )?;
        if let Some(loading_text) = loading_text {
            let len = loading_text.encode_utf16().count() as i32;
            draw_string(
                painter,
                font,
                &utf16(loading_text),
                [canvas[0] / 2 - len * 6 / 2, canvas[1] / 2 - 26],
                opaque(text_colour),
                -1,
            )?;
        }
    }
    Ok(())
}
