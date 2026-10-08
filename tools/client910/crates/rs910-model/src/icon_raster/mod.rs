//! The CPU triangle rasteriser that draws inventory item icons.
//!
//! An icon is a small (36x32) ARGB canvas with a depth buffer. The icon
//! builder projects a model's faces to canvas coordinates and fills them
//! in draw order here: flat, palette-shaded (Gouraud), true-colour shaded and
//! perspective-textured triangles, each optionally translucent.
//!
//! The pixels this produces are part of the client's look, so the arithmetic
//! is exact rather than idiomatic: coordinates are `f32` and rows are picked by
//! `(y + 0.5) as i32`; blends are integer channel maths on packed ARGB. Only
//! triangles with a corner outside the canvas width are clipped horizontally;
//! the spans of the others are trusted to stay inside their row.
//!
//! Layout: [`scan`] walks a triangle's edges row by row, [`attributes`] holds
//! what each fill mode interpolates, [`fill`] and [`textured`] turn a row's
//! span into pixels.

mod attributes;
mod fill;
mod scan;
#[cfg(test)]
mod tests;
mod textured;

use std::{collections::BTreeMap, sync::OnceLock};

use attributes::{Attributes, PerEdge, Planar};
use fill::{ChannelFill, FlatFill, PaletteFill};
use scan::{Corner, SortRule};
use textured::TexturedFill;

/// The pixel and depth buffers of the icon being drawn. Depth starts at
/// infinity; a fragment is drawn where its depth is smaller than the stored
/// one.
struct Canvas {
    width: i32,
    height: i32,
    pixels: Vec<i32>,
    depth: Vec<f32>,
}

/// One row of a triangle: the pixel index the row starts at and the span's
/// horizontal bounds (before clipping). `clip` says whether the bounds are
/// clamped to the canvas width; a span that is not clipped may run past the
/// end of its row.
struct Span {
    row_start: i32,
    left: i32,
    right: i32,
    clip: bool,
}

/// How much of what is already on the canvas shows through a fill: a level
/// out of 256, 0 being fully opaque. Level [`Translucency::DISTORT_LEVEL`] has
/// a special meaning for flat fills only: instead of colouring, it shifts
/// the pixels behind the face one pixel to the left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Translucency(i32);

impl Translucency {
    pub const OPAQUE: Self = Self(0);
    /// The level a flat fill reads as "distort the background".
    pub const DISTORT_LEVEL: i32 = 254;

    pub fn from_level(level: i32) -> Self {
        Self(level)
    }

    pub fn level(self) -> i32 {
        self.0
    }
}

/// A corner on the canvas: pixel position and the depth compared against the
/// depth buffer.
#[derive(Clone, Copy, Debug)]
pub struct ScreenPoint {
    pub x: f32,
    pub y: f32,
    pub depth: f32,
}

/// A textured face's corner: position and depth, the eye-space `w`
/// (perspective divisor), texture coordinates, and an ARGB light value whose
/// alpha is the face's opacity and whose colour channels tint the texel.
#[derive(Clone, Copy, Debug)]
pub struct TexturedPoint {
    pub at: ScreenPoint,
    pub w: f32,
    pub u: f32,
    pub v: f32,
    pub light: i32,
}

/// How a material's texel alpha is read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TextureAlpha {
    /// Texel alpha is ignored; the face's own opacity applies.
    Opaque,
    /// Texels above the material's threshold are opaque, the rest clear.
    Cutout,
    /// Texel alpha scales the face's opacity.
    Blended,
}

/// A material's texels as the icon path samples them.
pub struct IconTexture {
    /// `size * size` ARGB texels, or `None` for a textured material without
    /// a diffuse map (its faces are filled with the average colour instead).
    pub texels: Option<Vec<i32>>,
    pub size: i32,
    /// HSL16 average colour of the material.
    pub average_colour: i32,
    pub alpha: TextureAlpha,
    pub alpha_threshold: i32,
    /// Whether texture coordinates wrap (otherwise they clamp).
    pub repeat: bool,
}

/// The 65536-entry HSL16-to-ARGB table icons shade with. It is identical for
/// every icon, so it is built once.
fn palette() -> &'static [i32] {
    static PALETTE: OnceLock<Vec<i32>> = OnceLock::new();
    PALETTE.get_or_init(crate::colour::build_hsv_table)
}

/// The palette index of an HSL16 colour after the saturation-versus-lightness
/// adjustment models get before they are shaded.
fn adjusted_hsl(hsl: i32) -> i32 {
    i32::from(crate::colour::renormalise_saturation(hsl) as i16)
}

/// `colour` with every channel scaled by 255/256 (a zero-strength fog mix
/// still costs that much), alpha dropped.
fn fog_faded(colour: i32) -> i32 {
    let scale = |channels: i32, mask: i32, keep: u32| {
        ((channels & mask).wrapping_mul(255) & keep as i32) as u32
    };
    let low = scale(colour, 0xff00ff, 0xff00ff00);
    let high = scale(colour, 0xff00, 0xff0000);
    ((low | high) >> 8) as i32
}

/// An icon canvas and the materials registered for its textured faces. Draw
/// with the `fill_*` methods, then take the pixels.
pub struct IconRaster {
    canvas: Canvas,
    palette: &'static [i32],
    textures: BTreeMap<i32, IconTexture>,
}

impl IconRaster {
    pub fn new(width: i32, height: i32) -> Self {
        let cells = (width * height) as usize;
        Self {
            canvas: Canvas {
                width,
                height,
                pixels: vec![0; cells],
                depth: vec![f32::INFINITY; cells],
            },
            palette: palette(),
            textures: BTreeMap::new(),
        }
    }

    pub fn pixels(&self) -> &[i32] {
        &self.canvas.pixels
    }

    pub fn into_pixels(self) -> Vec<i32> {
        self.canvas.pixels
    }

    /// The ARGB colour of an HSL16 palette entry (only the low 16 bits of
    /// `index` count).
    pub fn palette_colour(&self, index: i32) -> i32 {
        self.palette[(index & 65535) as usize]
    }

    pub fn has_texture(&self, material: i32) -> bool {
        self.textures.contains_key(&(material & 65535))
    }

    pub fn add_texture(&mut self, material: i32, texture: IconTexture) {
        self.textures.insert(material & 65535, texture);
    }

    /// Whether a triangle sticks out of the canvas sideways, which is what
    /// switches on per-span horizontal clipping.
    fn sticks_out(&self, corners: &[Corner; 3]) -> bool {
        let width = self.canvas.width as f32;
        corners.iter().any(|c| c.x < 0.0 || c.x > width)
    }

    fn corners(points: [ScreenPoint; 3]) -> [Corner; 3] {
        points.map(|p| Corner { x: p.x, y: p.y })
    }

    /// A triangle of one ARGB colour.
    pub fn fill_flat(&mut self, points: [ScreenPoint; 3], colour: i32, translucency: Translucency) {
        let corners = Self::corners(points);
        let Some(mut plane) = Planar::new(corners, [points.map(|p| p.depth)]) else {
            return;
        };
        let clip = self.sticks_out(&corners);
        let across = plane.across;
        let canvas = &mut self.canvas;
        scan::scan(
            corners,
            canvas.width,
            canvas.height,
            SortRule::Planar,
            &mut plane,
            |row_start, left, right, row| {
                let span = Span {
                    row_start,
                    left,
                    right,
                    clip,
                };
                let fill = FlatFill {
                    colour,
                    translucency,
                    depth: row[0],
                    depth_step: across[0],
                };
                fill::flat_span(canvas, &span, &fill);
            },
        );
    }

    /// A triangle shaded by interpolating an HSL16 palette index between its
    /// corners.
    pub fn fill_palette_shaded(
        &mut self,
        points: [ScreenPoint; 3],
        palette_index: [i32; 3],
        translucency: Translucency,
    ) {
        let corners = Self::corners(points);
        let shade = palette_index.map(|i| i as f32);
        let Some(mut plane) = Planar::new(corners, [shade, points.map(|p| p.depth)]) else {
            return;
        };
        let clip = self.sticks_out(&corners);
        let across = plane.across;
        let palette = self.palette;
        let canvas = &mut self.canvas;
        scan::scan(
            corners,
            canvas.width,
            canvas.height,
            SortRule::Planar,
            &mut plane,
            |row_start, left, right, row| {
                let span = Span {
                    row_start,
                    left,
                    right,
                    clip,
                };
                let fill = PaletteFill {
                    palette,
                    translucency,
                    shade: row[0],
                    shade_step: across[0],
                    depth: row[1],
                    depth_step: across[1],
                };
                fill::palette_span(canvas, &span, &fill);
            },
        );
    }

    /// A triangle shaded by interpolating each RGB channel between its
    /// corners' colours (alpha bytes are ignored: the result is opaque before
    /// `translucency` applies).
    pub fn fill_rgb_shaded(
        &mut self,
        points: [ScreenPoint; 3],
        colours: [i32; 3],
        translucency: Translucency,
    ) {
        let corners = Self::corners(points);
        let channel = |mask: i32| colours.map(|c| (c & mask) as f32);
        let Some(mut plane) = Planar::new(
            corners,
            [
                points.map(|p| p.depth),
                channel(16711680),
                channel(65280),
                channel(255),
            ],
        ) else {
            return;
        };
        let clip = self.sticks_out(&corners);
        let across = plane.across;
        let canvas = &mut self.canvas;
        scan::scan(
            corners,
            canvas.width,
            canvas.height,
            SortRule::Planar,
            &mut plane,
            |row_start, left, right, row| {
                let span = Span {
                    row_start,
                    left,
                    right,
                    clip,
                };
                let fill = ChannelFill {
                    translucency,
                    depth: row[0],
                    depth_step: across[0],
                    red: row[1],
                    red_step: across[1],
                    green: row[2],
                    green_step: across[2],
                    blue: row[3],
                    blue_step: across[3],
                };
                fill::channel_span(canvas, &span, &fill);
            },
        );
    }

    /// A perspective-textured triangle of a material registered with
    /// [`IconRaster::add_texture`]. A material without a diffuse map is
    /// filled with its average colour instead; an unregistered material draws
    /// nothing.
    pub fn fill_textured(&mut self, points: [TexturedPoint; 3], material: i32) {
        let id = material & 65535;
        let Some(texture) = self.textures.get(&id) else {
            return;
        };
        let Some(texels) = &texture.texels else {
            let average = texture.average_colour;
            self.fill_average_colour(points, average);
            return;
        };
        let corners = Self::corners(points.map(|p| p.at));
        let attributes: [Attributes; 3] = points.map(|p| {
            [
                1.0 / p.at.depth,
                1.0 / p.w,
                p.u / p.w,
                p.v / p.w,
                ((p.light >> 24) & 255) as f32,
                ((p.light >> 16) & 255) as f32,
                ((p.light >> 8) & 255) as f32,
                (p.light & 255) as f32,
            ]
        });
        let mut edges = PerEdge::new(corners.map(|c| c.y), attributes);
        let clip = self.sticks_out(&corners);
        let canvas = &mut self.canvas;
        scan::scan(
            corners,
            canvas.width,
            canvas.height,
            SortRule::Perspective,
            &mut edges,
            |row_start, left, right, (start, end)| {
                let span = Span {
                    row_start,
                    left,
                    right,
                    clip,
                };
                let fill = TexturedFill {
                    texture,
                    texels,
                    start,
                    end,
                };
                textured::textured_span(canvas, &span, &fill);
            },
        );
    }

    /// The stand-in for a textured face whose material has no texels: one
    /// flat colour, the material's average shaded by the first corner's light.
    fn fill_average_colour(&mut self, points: [TexturedPoint; 3], average_colour: i32) {
        let light = points[0].light;
        let translucency = Translucency::from_level(255 - ((light >> 24) & 255));
        let average = self.palette[(adjusted_hsl(average_colour) & 65535) as usize];
        let channel = |shift: u32| ((light >> shift) & 255).wrapping_mul((average >> shift) & 255);
        let colour = ((((channel(16) & 65280) | 16711680) << 8) | (channel(8) & 65280))
            | (((light & 255).wrapping_mul(average & 255)) >> 8);
        let faded = fog_faded(colour);
        self.fill_rgb_shaded(points.map(|p| p.at), [faded; 3], translucency);
    }
}
