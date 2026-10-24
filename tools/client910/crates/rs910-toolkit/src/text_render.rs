//! Glyph and batch-vertex quads. Painter-ordered textured quads on the
//! caller's device and target; no separate surface or UI-owned logic clock.
use crate::{font_layout::Draw, font_metrics::Metrics};

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub colour: [u8; 4],
}

/// The clipped, normalized quad. Caller retains texture identity separately.
/// Coordinates are in render-target pixels; GLX's half-pixel offset is zero.
pub fn quad(
    mut rect: [f32; 4],
    mut uv: [f32; 4],
    colour: u32,
    clip: [i32; 4],
    size: [u32; 2],
) -> Option<[Vertex; 4]> {
    if colour >> 24 == 0 {
        return None;
    }
    let c = clip.map(|v| v as f32);
    if rect[0] > c[2] || rect[1] > c[3] || rect[2] < c[0] || rect[3] < c[1] {
        return None;
    }
    let (w, h, du, dv) = (
        rect[2] - rect[0],
        rect[3] - rect[1],
        uv[2] - uv[0],
        uv[3] - uv[1],
    );
    if rect[0] < c[0] {
        uv[0] += (c[0] - rect[0]) / w * du;
        rect[0] = c[0];
    }
    if rect[1] < c[1] {
        uv[1] += (c[1] - rect[1]) / h * dv;
        rect[1] = c[1];
    }
    if rect[2] > c[2] {
        uv[2] -= (rect[2] - c[2]) / w * du;
        rect[2] = c[2];
    }
    if rect[3] > c[3] {
        uv[3] -= (rect[3] - c[3]) / h * dv;
        rect[3] = c[3];
    }
    let x = |v: f32| v / size[0] as f32 * 2. - 1.;
    let y = |v: f32| (1. - v / size[1] as f32) * 2. - 1.;
    let colour = [
        (colour >> 16) as u8,
        (colour >> 8) as u8,
        colour as u8,
        (colour >> 24) as u8,
    ];
    Some([
        Vertex {
            position: [x(rect[0]), y(rect[1])],
            uv: [uv[0], uv[1]],
            colour,
        },
        Vertex {
            position: [x(rect[2]), y(rect[1])],
            uv: [uv[2], uv[1]],
            colour,
        },
        Vertex {
            position: [x(rect[0]), y(rect[3])],
            uv: [uv[0], uv[3]],
            colour,
        },
        Vertex {
            position: [x(rect[2]), y(rect[3])],
            uv: [uv[2], uv[3]],
            colour,
        },
    ])
}

/// The batched path adds the unsigned vertical offset. A masked glyph is
/// empty in this client and therefore emits nothing.
pub fn glyph(m: &Metrics, draw: &Draw, clip: [i32; 4], size: [u32; 2]) -> Option<[Vertex; 4]> {
    let Draw::Glyph {
        code,
        x,
        y,
        colour,
        masked: false,
        ..
    } = *draw
    else {
        return None;
    };
    let c = code as usize;
    let y = y.wrapping_add(m.bearings[c] as i32) as f32;
    let v = m.glyph_vertices(c);
    quad(
        [
            x as f32,
            y,
            x as f32 + m.advances[c] as f32,
            y + m.widths[c] as f32,
        ],
        [v[0][3], v[0][4], v[2][3], v[2][4]],
        colour as u32,
        clip,
        size,
    )
}
