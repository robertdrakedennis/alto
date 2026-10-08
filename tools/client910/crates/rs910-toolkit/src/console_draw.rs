//! The developer console as a toolkit draws it: the plan of its draw calls
//! (clip resets, blended fills, the default fonts' strings, the caret line)
//! and [`ConsoleView`], what the GPU
//! console renderer and the software toolkit read of the console. Split out
//! of client910's `console` in Phase 3.2 so the renderers do not name the
//! UI's `Console`, which implements [`ConsoleView`].
use crate::font_metrics::Metrics;
use rs910_config::utf16_text::Text;

/// What a toolkit reads of the developer console: whether it is open
/// and its draw-call plan at canvas width
/// `width` with the default fonts' metrics (`p11`, `p12`, `b12`).
pub trait ConsoleView {
    fn open(&self) -> bool;
    fn draw(
        &self,
        width: i32,
        cycle: i32,
        focused: bool,
        fonts: [&Metrics; 3],
    ) -> anyhow::Result<Vec<Draw>>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Font {
    P11,
    P12,
    B12,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Draw {
    Clip([i32; 4]),
    Fill {
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        colour: i32,
        blend: i32,
    },
    Text {
        font: Font,
        text: Text,
        x: i32,
        y: i32,
        right: bool,
        colour: i32,
        shadow: i32,
    },
    Line {
        x: i32,
        y: i32,
        length: i32,
        vertical: bool,
        colour: i32,
    },
    ResetClip,
}
