//! The render scale (`ModernSettings::render_scale`, lane P4-GPU): the
//! scene renders at a fraction of the scene viewport's pixels and the post
//! chain's display frame is upscaled into the viewport, under the faithful
//! UI, which stays at the drawable's own resolution (the shell draws it
//! over the frame after this renderer).
//!
//! At scale 1 nothing here applies: every target is the frame's size and
//! the scene viewport keeps its place in it, the frame as before. At another
//! scale the scene's targets are the scaled viewport's size, with the
//! viewport at their origin; the camera keeps the viewport's own size (its
//! projection is in the viewport's pixels, `SceneCamera::projection`), so
//! the scaled image is the same view, and the upscale maps it back onto the
//! viewport pixel for pixel: a point of the world lands where the full
//! resolution frame (and the client's picking, which works in the
//! viewport's coordinates) puts it.

/// What decides the render scale of a frame: the display's scale factor and
/// the shell's saved choice, under the setting's own value
/// ([`crate::settings::ModernSettings::render_scale`], the variable
/// override). Nothing set: [`RenderScale::auto`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Display {
    pub(crate) scale_factor: f64,
    pub(crate) saved: Option<RenderScale>,
    /// The percentage last logged, so a change is logged once.
    logged: Option<u16>,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            scale_factor: 1.0,
            saved: None,
            logged: None,
        }
    }
}

impl Display {
    /// The scale of a frame whose scene viewport has `viewport_pixels`
    /// pixels, `setting` being the renderer's settings' value.
    pub(crate) fn resolve(
        &mut self,
        setting: Option<RenderScale>,
        viewport_pixels: u64,
    ) -> RenderScale {
        let (scale, why) = match (setting, self.saved) {
            (Some(s), _) => (s, "setting".to_string()),
            (None, Some(s)) => (s, "saved choice".to_string()),
            (None, None) => (
                RenderScale::auto(self.scale_factor, viewport_pixels),
                format!(
                    "automatic: scale factor {:.2}, scene viewport {viewport_pixels} pixels",
                    self.scale_factor
                ),
            ),
        };
        if self.logged != Some(scale.as_percent()) {
            self.logged = Some(scale.as_percent());
            log::info!("[modern] render scale {}% ({why})", scale.as_percent());
        }
        scale
    }
}

use crate::settings::RenderScale;

/// A frame's scene viewport at the render scale.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scaled {
    /// The frame's target size and the viewport and scissor in it (the
    /// shell's, what the upscale writes).
    pub(crate) native_size: [u32; 2],
    pub(crate) native_rect: [i32; 4],
    pub(crate) native_clip: [i32; 4],
    /// The scene's targets' size, and the viewport and scissor in them.
    pub(crate) size: [u32; 2],
    pub(crate) rect: [i32; 4],
    pub(crate) clip: [i32; 4],
}

impl Scaled {
    /// The viewport `rect` (scissor `clip`) of a `size` frame at `percent`
    /// of its pixels per axis (`None`: 100, the frame's own targets).
    pub(crate) fn of(percent: u16, size: [u32; 2], rect: [i32; 4], clip: [i32; 4]) -> Option<Self> {
        if percent == 100 {
            return None;
        }
        let [x, y, w, h] = rect;
        let s = f64::from(percent) / 100.0;
        // The viewport times the scale, truncated, at least 1.
        let sw = ((f64::from(w) * s) as i32).max(1);
        let sh = ((f64::from(h) * s) as i32).max(1);
        // The scissor in the scaled viewport, rounded outwards (the
        // upscale's own scissor is the frame's).
        let (kx, ky) = (f64::from(sw) / f64::from(w), f64::from(sh) / f64::from(h));
        let [l, t, r, b] = clip;
        let lo =
            |v: i32, o: i32, k: f64, n: i32| ((f64::from(v - o) * k).floor() as i32).clamp(0, n);
        let hi =
            |v: i32, o: i32, k: f64, n: i32| ((f64::from(v - o) * k).ceil() as i32).clamp(0, n);
        Some(Self {
            native_size: size,
            native_rect: rect,
            native_clip: clip,
            size: [sw as u32, sh as u32],
            rect: [0, 0, sw, sh],
            clip: [
                lo(l, x, kx, sw),
                lo(t, y, ky, sh),
                hi(r, x, kx, sw),
                hi(b, y, ky, sh),
            ],
        })
    }

    /// Viewport pixels per scene pixel, per axis.
    pub(crate) fn ratio(&self) -> [f32; 2] {
        [
            self.native_rect[2] as f32 / self.rect[2] as f32,
            self.native_rect[3] as f32 / self.rect[3] as f32,
        ]
    }
}
