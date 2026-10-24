//! The CPU font resource (a decoded font): metrics, the glyph atlas pixels
//! and the sheet's paletted/translucent class. Every toolkit creates its own
//! font from it (the GPU atlas upload). Split out of client910's
//! `ui_fonts` (Phase 3.1); `ui_fonts` (the font cache) re-exports it.
use crate::{font_atlas::Atlas, font_metrics::Metrics, sprite_sheet::SpriteSheet};
use anyhow::{Context, Result};
use std::rc::Rc;

#[derive(Debug)]
pub struct Font {
    pub metrics: Rc<Metrics>,
    pub atlas: Atlas,
    pub monochrome: bool,
    /// Whether the font sheet is paletted / translucent (the font-class
    /// choice of a software rasteriser).
    pub paletted: bool,
    pub translucent: bool,
}

impl Font {
    pub fn create(
        bytes: Option<Vec<u8>>,
        metrics: Option<Rc<Metrics>>,
        monochrome: bool,
    ) -> Result<Rc<Self>> {
        let sheet = SpriteSheet::decode(&bytes.context("missing font sprites")?)?;
        let metrics = metrics.context("missing font metrics")?;
        let atlas = Atlas::new(&metrics, &sheet, monochrome)?;
        let (paletted, translucent) = match &sheet {
            // An all-255 alpha plane is discarded.
            SpriteSheet::Paletted { sprites, .. } => (
                true,
                sprites
                    .first()
                    .and_then(|s| s.alpha.as_ref())
                    .is_some_and(|a| a.iter().any(|v| *v != 255)),
            ),
            SpriteSheet::Full { has_alpha, .. } => (false, *has_alpha),
        };
        Ok(Rc::new(Self {
            metrics,
            atlas,
            monochrome,
            paletted,
            translucent,
        }))
    }
}
