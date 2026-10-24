//! Textured span fill: perspective-correct texel lookup, per-pixel
//! translucency and light tint.

use super::attributes::{add, scaled_difference, subtract_scaled, Attributes};
use super::{Canvas, IconTexture, Span, TextureAlpha};

/// Where the eight interpolated attributes sit (see
/// `attributes::TEXTURED_ATTRIBUTES`).
const RECIPROCAL_DEPTH: usize = 0;
const RECIPROCAL_W: usize = 1;
const U_OVER_W: usize = 2;
const V_OVER_W: usize = 3;
const ALPHA: usize = 4;
const RED: usize = 5;
const GREEN: usize = 6;
const BLUE: usize = 7;

/// The texel a perspective-correct position falls on.
fn texel_index(scaled: f32, size: i32, repeat: bool) -> i32 {
    let coordinate = scaled as i32;
    if repeat {
        coordinate & (size - 1)
    } else {
        if coordinate < 0 {
            0
        } else if coordinate > size - 1 {
            size - 1
        } else {
            coordinate
        }
    }
}

/// The face colour for a texel under the interpolated light tint. The light
/// channels are 0..=255 multipliers of a 256 scale; red is built in the
/// alpha-ready position, so the result always has alpha 0xff.
fn tinted(texel: i32, red: f32, green: f32, blue: f32) -> i32 {
    (((((((texel >> 16) & 255) as f32) * red) as i32 & 65280) | 16711680) << 8)
        | ((((((texel >> 8) & 255) as f32) * green) as i32) & 65280)
        | (((((texel & 255) as f32) * blue) as i32) >> 8)
}

/// What a textured span samples and interpolates: the material, its texels,
/// and the attributes at the span's two ends.
pub(super) struct TexturedFill<'a> {
    pub texture: &'a IconTexture,
    pub texels: &'a [i32],
    pub start: &'a Attributes,
    pub end: &'a Attributes,
}

pub(super) fn textured_span(canvas: &mut Canvas, span: &Span, fill: &TexturedFill<'_>) {
    let TexturedFill {
        texture,
        texels,
        start,
        end,
    } = *fill;
    let (mut left, mut right) = (span.left, span.right);
    let inverse_length = 1.0f32 / ((right.wrapping_sub(left)) as f32);
    let step = scaled_difference(end, start, inverse_length);
    let mut at = *start;
    if span.clip {
        if right > canvas.width {
            right = canvas.width;
        }
        if left < 0 {
            subtract_scaled(&mut at, left as f32, &step);
            left = 0;
        }
    }
    if left >= right {
        return;
    }
    let size = texture.size;
    let mut index = span.row_start.wrapping_add(left);
    for _ in 0..right - left {
        let depth = 1.0f32 / at[RECIPROCAL_DEPTH];
        let w = 1.0f32 / at[RECIPROCAL_W];
        let i = index as usize;
        let visible = canvas.depth.get(i).is_some_and(|stored| depth < *stored);
        if visible {
            let u = texel_index((at[U_OVER_W] * w) * (size as f32), size, texture.repeat);
            let v = texel_index((at[V_OVER_W] * w) * (size as f32), size, texture.repeat);
            let Some(&texel) = texels.get(size.wrapping_mul(v).wrapping_add(u) as usize) else {
                index = index.wrapping_add(1);
                add(&mut at, &step);
                continue;
            };
            let texel_alpha = (texel >> 24) & 255;
            let alpha = match texture.alpha {
                TextureAlpha::Blended => ((texel_alpha as f32 * at[ALPHA]) / 255.0f32) as i32,
                TextureAlpha::Opaque => at[ALPHA] as i32,
                TextureAlpha::Cutout => {
                    if texel_alpha > texture.alpha_threshold {
                        255
                    } else {
                        0
                    }
                }
            };
            if alpha != 0 {
                let colour = tinted(texel, at[RED], at[GREEN], at[BLUE]);
                if alpha == 255 {
                    canvas.pixels[i] = colour;
                } else {
                    let background = canvas.pixels[i];
                    let behind = 255i32.wrapping_sub(alpha);
                    canvas.pixels[i] = ((((colour & 16711935).wrapping_mul(alpha))
                        .wrapping_add((background & 16711935).wrapping_mul(behind))
                        & -16711936)
                        .wrapping_add(
                            ((colour & 65280).wrapping_mul(alpha))
                                .wrapping_add((background & 65280).wrapping_mul(behind))
                                & 16711680,
                        ))
                    .wrapping_shr(8);
                }
                canvas.depth[i] = depth;
            }
        }
        index = index.wrapping_add(1);
        add(&mut at, &step);
    }
}
