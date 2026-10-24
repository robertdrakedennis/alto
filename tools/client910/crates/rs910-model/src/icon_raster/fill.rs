//! Span fills for flat, palette-Gouraud and true-colour Gouraud triangles,
//! and the colour blending they share.
//!
//! Every span is depth tested against, and written to, the canvas depth
//! buffer (smaller depth wins). Pixel writes never touch the alpha byte of a
//! translucent result: the icon path clears alpha afterwards.

use super::{Canvas, Span, Translucency};

/// The share of the background kept when a fill is translucent, out of 256.
/// Level 0 (opaque) never blends.
fn background_share(translucency: Translucency) -> i32 {
    translucency.level()
}

/// `colour` scaled by `scale / 256` per channel, alpha byte dropped.
fn scaled(colour: i32, scale: i32) -> i32 {
    ((((colour & 0xff00ff).wrapping_mul(scale)) >> 8) & 0xff00ff)
        .wrapping_add((((colour & 0xff00).wrapping_mul(scale)) >> 8) & 0xff00)
}

/// A translucent source over a background: the background scaled by `keep`
/// plus the source already scaled by `256 - keep`.
fn over(background: i32, keep: i32, source_scaled: i32) -> i32 {
    ((((background & 0xff00).wrapping_mul(keep)) >> 8) & 0xff00)
        .wrapping_add((((background & 0xff00ff).wrapping_mul(keep)) >> 8) & 0xff00ff)
        .wrapping_add(source_scaled)
}

/// The horizontal extent of a span after clipping to the canvas width (only
/// when the span asks for it), or `None` if it is empty.
fn clipped(canvas: &Canvas, span: &Span) -> Option<(i32, i32)> {
    let (mut left, mut right) = (span.left, span.right);
    if span.clip {
        if right > canvas.width {
            right = canvas.width;
        }
        if left < 0 {
            left = 0;
        }
    }
    (left < right).then_some((left, right))
}

/// One pixel of a depth-tested fill: `write` gets the pixel index when
/// `depth` is nearer than what the buffer holds, and the buffer is updated.
#[inline(always)]
fn depth_tested(canvas: &mut Canvas, index: i32, depth: f32, write: impl FnOnce(&mut i32)) {
    let i = index as usize;
    if let (Some(stored), Some(pixel)) = (canvas.depth.get_mut(i), canvas.pixels.get_mut(i)) {
        if depth < *stored {
            write(pixel);
            *stored = depth;
        }
    }
}

/// How a flat span is filled: one colour. `depth` is the depth at pixel 0 and
/// `depth_step` its change per pixel.
pub(super) struct FlatFill {
    pub colour: i32,
    pub translucency: Translucency,
    pub depth: f32,
    pub depth_step: f32,
}

pub(super) fn flat_span(canvas: &mut Canvas, span: &Span, fill: &FlatFill) {
    let Some((left, right)) = clipped(canvas, span) else {
        return;
    };
    let mut index = (left - 1).wrapping_add(span.row_start);
    let mut depth = (left as f32 * fill.depth_step) + fill.depth;
    let count = right - left;
    let level = background_share(fill.translucency);
    if level == 0 {
        for _ in 0..count {
            index = index.wrapping_add(1);
            depth_tested(canvas, index, depth, |p| *p = fill.colour);
            depth += fill.depth_step;
        }
    } else if level != Translucency::DISTORT_LEVEL {
        let source = scaled(fill.colour, 256 - level);
        for _ in 0..count {
            index = index.wrapping_add(1);
            depth_tested(canvas, index, depth, |p| *p = over(*p, level, source));
            depth += fill.depth_step;
        }
    } else if left != 0 && right <= canvas.width.wrapping_sub(1) {
        // Distortion: pixels that pass the depth test take their right
        // neighbour's colour (the row shifts one pixel left). It only runs
        // for spans clear of both canvas edges, and it leaves depth alone.
        for _ in 0..count {
            index = index.wrapping_add(1);
            let i = index as usize;
            let passes = canvas.depth.get(i).is_some_and(|stored| depth < *stored);
            if passes {
                if let (Some(&neighbour), true) = (canvas.pixels.get(i), i > 0) {
                    canvas.pixels[i - 1] = neighbour;
                }
            }
            depth += fill.depth_step;
        }
    }
}

/// How a palette-shaded span is filled: by a palette index that varies along
/// it.
pub(super) struct PaletteFill<'a> {
    pub palette: &'a [i32],
    pub translucency: Translucency,
    /// Palette index, as a float, at pixel 0 and its change per pixel.
    pub shade: f32,
    pub shade_step: f32,
    pub depth: f32,
    pub depth_step: f32,
}

pub(super) fn palette_span(canvas: &mut Canvas, span: &Span, fill: &PaletteFill<'_>) {
    let Some((left, right)) = clipped(canvas, span) else {
        return;
    };
    let mut index = (left - 1).wrapping_add(span.row_start);
    let mut shade = (left as f32 * fill.shade_step) + fill.shade;
    let mut depth = (left as f32 * fill.depth_step) + fill.depth;
    let level = background_share(fill.translucency);
    let colour_at = |shade: f32| fill.palette[((shade as i32) & 65535) as usize];
    for _ in 0..right - left {
        index = index.wrapping_add(1);
        if level == 0 {
            depth_tested(canvas, index, depth, |p| *p = colour_at(shade));
        } else {
            depth_tested(canvas, index, depth, |p| {
                *p = over(*p, level, scaled(colour_at(shade), 256 - level));
            });
        }
        depth += fill.depth_step;
        shade += fill.shade_step;
    }
}

/// How a channel-shaded span is filled: by three colour channels that vary
/// along it. The channels
/// are floats holding each channel in place (red as 0xff0000-scaled, and so
/// on), so they are masked, not shifted, when a pixel is made.
pub(super) struct ChannelFill {
    pub translucency: Translucency,
    pub depth: f32,
    pub depth_step: f32,
    pub red: f32,
    pub red_step: f32,
    pub green: f32,
    pub green_step: f32,
    pub blue: f32,
    pub blue_step: f32,
}

pub(super) fn channel_span(canvas: &mut Canvas, span: &Span, fill: &ChannelFill) {
    let Some((left, right)) = clipped(canvas, span) else {
        return;
    };
    let mut index = (left - 1).wrapping_add(span.row_start);
    let start = left as f32;
    let mut depth = (start * fill.depth_step) + fill.depth;
    let mut red = (start * fill.red_step) + fill.red;
    let mut green = (start * fill.green_step) + fill.green;
    let mut blue = (start * fill.blue_step) + fill.blue;
    let level = background_share(fill.translucency);
    for _ in 0..right - left {
        index = index.wrapping_add(1);
        let colour = (((red as i32) & 16711680) | -16777216 | ((green as i32) & 65280))
            | ((blue as i32) & 255);
        if level == 0 {
            depth_tested(canvas, index, depth, |p| *p = colour);
        } else {
            depth_tested(canvas, index, depth, |p| {
                *p = over(*p, level, scaled(colour, 256 - level));
            });
        }
        depth += fill.depth_step;
        red += fill.red_step;
        green += fill.green_step;
        blue += fill.blue_step;
    }
}
