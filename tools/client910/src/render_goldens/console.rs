//! the developer console's draw as the
//! toolkit call sequence every backend receives (`ConsoleView::draw`, the
//! `console_draw::Draw` plan the GPU console renderer consumes). Until lane
//! DROP-SW this was checked through the software toolkit's pixels against
//! the reference toolkit; the call sequence those pixels encoded is now
//! asserted directly.
use crate::console_draw::{ConsoleView, Draw, Font};
use std::rc::Rc;

/// The default fonts (the monochrome p11/p12/b12 full fonts)
/// through the UI's font provider.
fn default_fonts(pack: &crate::cache::Pack) -> [Rc<crate::font::Font>; 3] {
    let fonts = crate::ui_fonts::Fonts::from_pack(pack.clone(), None).unwrap();
    let ids = fonts.ids.clone().expect("default font ids");
    let get = |i: usize| fonts.get_font(ids[i], true, true).unwrap().unwrap();
    [get(0), get(1), get(2)]
}

/// The console draw calls for the scene below: canvas
/// width 400, lines 1-4 of 5 shown (scroll offset 1), the default fonts' cache
/// metrics (p12 and b12: ascent 12, descent 4, row height 18; b12's width of
/// "--> hello" is 59) and the caret colour of the logic cycle modulo 30.
fn recorded_calls(opacity: i32, caret: i32) -> Vec<Draw> {
    let text = crate::console::text;
    let colour = opacity << 24 | 0x332277;
    let shadow = 0xff000000u32 as i32;
    let p12 = |t: &str, x, y| Draw::Text {
        font: Font::P12,
        text: text(t),
        x,
        y,
        right: false,
        colour: -1,
        shadow,
    };
    vec![
        // The bounds reset and the background.
        Draw::Clip([0, 0, 400, 350]),
        Draw::Fill {
            x: 0,
            y: 0,
            width: 400,
            height: 350,
            colour,
            blend: 1,
        },
        // the scrollbar: track 346 - 18 - 4, thumb 19 * 324 / 23,
        // at 4 + 3 * (324 - 267) / 4.
        Draw::Fill {
            x: 384,
            y: 46,
            width: 12,
            height: 267,
            colour,
            blend: 2,
        },
        // one clip and p12 string per '\b' column, bottom row first
        // at 350 - 18 - 2 - 4; the directlogin password is masked.
        Draw::Clip([8, 0, 125, 350]),
        p12("alpha", 8, 326),
        Draw::Clip([133, 0, 250, 350]),
        p12("beta", 133, 326),
        Draw::Clip([258, 0, 375, 350]),
        p12("gamma", 258, 326),
        Draw::Clip([8, 0, 376, 350]),
        p12("directlogin bob ******", 8, 308),
        Draw::Clip([8, 0, 376, 350]),
        p12("<col=ff0000>red</col> text", 8, 290),
        Draw::Clip([8, 0, 376, 350]),
        p12("oldest", 8, 272),
        // the version, right-aligned in p11.
        Draw::Text {
            font: Font::P11,
            text: text("910 1"),
            x: 375,
            y: 330,
            right: true,
            colour: -1,
            shadow,
        },
        // the entry line and the entry in b12.
        Draw::Clip([0, 0, 400, 350]),
        Draw::Line {
            x: 0,
            y: 332,
            length: 400,
            vertical: false,
            colour: -1,
        },
        Draw::Text {
            font: Font::B12,
            text: text("--> hello world"),
            x: 10,
            y: 345,
            right: false,
            colour: -1,
            shadow,
        },
        // the caret after "--> hello".
        Draw::Line {
            x: 69,
            y: 335,
            length: 12,
            vertical: true,
            colour: caret,
        },
        // The clip reset.
        Draw::ResetClip,
    ]
}

/// The console draw against the recorded call sequence with the default p11/p12/b12 full fonts:
/// tab-split rows, a `directlogin` redaction, a coloured tag, the
/// scrollbar, entry line and caret, at two opacities/caret phases.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn developer_console_draws_the_recorded_call_sequence() {
    let pack = crate::test_support::require_pack("client.fontmetrics.js5");
    let fonts = default_fonts(&pack);
    let metrics: [&crate::font_metrics::Metrics; 3] =
        [&fonts[0].metrics, &fonts[1].metrics, &fonts[2].metrics];
    let text = crate::console::text;
    let mut lines = vec![vec![]; 500];
    for (i, l) in [
        "newest line",
        "alpha\u{8}beta\u{8}gamma",
        "directlogin bob secret",
        "<col=ff0000>red</col> text",
        "oldest",
    ]
    .iter()
    .enumerate()
    {
        lines[i] = text(l);
    }
    // the console's row heights (p12, b12).
    let row = |m: &crate::font_metrics::Metrics| m.ascent + m.descent + 2;
    for (opacity, cycle, caret) in [(200, 20, 0xffffff), (60, 3, -1)] {
        let c = crate::console::Console {
            open: true,
            opacity,
            entry: text("hello world"),
            cursor: 5,
            lines: Some(lines.clone()),
            count: 5,
            scroll: 1,
            row_height: row(metrics[1]),
            entry_height: row(metrics[2]),
            ..Default::default()
        };
        let plan = ConsoleView::draw(&c, 400, cycle, true, metrics).unwrap();
        assert_eq!(
            plan,
            recorded_calls(opacity, caret),
            "opacity {opacity}, cycle {cycle}"
        );
    }
}
