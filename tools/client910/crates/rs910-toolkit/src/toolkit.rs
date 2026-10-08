//! The [`Toolkit`] trait: the rendering boundary between the retained UI's
//! recorded 2D calls and a backend that draws them (target-architecture.md
//! §2.4, trait 1; programme §5 Phase 3.1).
//!
//! # Surface
//!
//! Derived from what both existing backends already implement, not
//! invented: the GPU painter (`ui_paint::Painter` + client910's
//! `ui_paint_gpu`) implements it, as did the software toolkit (removed in lane
//! DROP-SW). The
//! subset the client calls per frame through the display list is exactly the
//! [`Op`] variants, so the trait has one method per variant:
//!
//! | method | draws | `Op` |
//! |---|---|---|
//! | [`reset_bounds`](Toolkit::reset_bounds) | reset the clip | `ResetBounds` |
//! | [`set_bounds`](Toolkit::set_bounds) | intersect the clip | `SetBounds` |
//! | [`fill_rectangle`](Toolkit::fill_rectangle) | a filled rectangle | `Fill` |
//! | [`draw_rectangle`](Toolkit::draw_rectangle) | a rectangle outline | `Outline` |
//! | [`draw_line`](Toolkit::draw_line) | a line, one pixel or wider | `Line` |
//! | [`draw_horizontal_line`](Toolkit::draw_horizontal_line) | a horizontal line | `HorizontalLine` |
//! | [`draw_dashed_line`](Toolkit::draw_dashed_line) | a dashed line | `DashedLine` |
//! | [`fill_masked`](Toolkit::fill_masked) | a fill through a mask | `MaskedFill` |
//! | [`draw_sprite`](Toolkit::draw_sprite) | a sprite, plain or tinted | `Sprite` |
//! | [`draw_sprite_scaled`](Toolkit::draw_sprite_scaled) | a scaled, tinted sprite | `Scaled` |
//! | [`draw_sprite_tiled`](Toolkit::draw_sprite_tiled) | a tiled sprite | `Tiled` |
//! | [`draw_component_sprite`](Toolkit::draw_component_sprite) | a graphic component's sprite | `Component` |
//! | [`draw_rotated`](Toolkit::draw_rotated) | a rotated, scaled image | `Rotated` |
//! | [`draw_affine`](Toolkit::draw_affine) | a parallelogram image | `Affine` |
//! | [`draw_sprite_masked`](Toolkit::draw_sprite_masked) | a sprite through a mask | `MaskedSprite` |
//! | [`draw_glyph`](Toolkit::draw_glyph) | one font glyph, plain or masked | `Glyph` |
//!
//! Every draw names the CPU resource ([`Sprite`], [`Font`]) and each backend
//! keeps its own derived object in a cache keyed by the resource's `Rc`
//! identity (GPU textures in `ui_paint_gpu`).
//!
//! # Implementations
//!
//! - The GPU painter ([`Painter`], below): each method is the matching
//!   `Painter` call, which records the [`Op`] and emits the batch quads the
//!   GPU renderer uploads. The UI draws through `Painter`'s own methods; a
//!   replay through the trait reproduces the same recording and quads for
//!   every op (tested), except two calls whose GPU geometry is not an op of
//!   its own: `Painter::masked_native_tinted` records the plain call
//!   (`Op::Sprite`, unmasked) but masks its GPU quad, and
//!   `Painter::framebuffer_sprite` composites a layer without recording an
//!   op (the layer's calls are appended to the recording).
//! - [`NullToolkit`]: records the calls and answers nothing; the headless
//!   backend for tests (its [`NullToolkit::digest`] is the 2D op-stream
//!   hash).
//!
//! # Errors
//!
//! Methods return `Result` because the GPU painter's sprite geometry raises
//! division-by-zero and non-terminating-tile errors (`sprite_draw`'s division
//! and tile-period checks).
use crate::{
    font::Font,
    font_layout::Draw,
    sprite::Sprite,
    ui_component_fields::Fields,
    ui_paint::{ComponentSprite, Image, MaskRef, Op, Painter, Recording, RotatedImage},
};
use anyhow::Result;
use std::{collections::BTreeMap, rc::Rc};

/// The per-frame 2D API and the sprite/font draws the retained UI makes, one
/// method per [`Op`]. Arguments are in canvas pixels; colours are ARGB ints.
pub trait Toolkit {
    /// Reset the clip to `[x0, y0, x1, y1]`.
    fn reset_bounds(&mut self, bounds: [i32; 4]) -> Result<()>;
    /// Intersect the clip with `[x0, y0, x1, y1]`.
    fn set_bounds(&mut self, bounds: [i32; 4]) -> Result<()>;
    /// Fill `[x, y, w, h]` with `colour`.
    fn fill_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()>;
    /// Outline `[x, y, w, h]` in `colour`.
    fn draw_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()>;
    /// A one-pixel line, or a wider one when `width != 1`.
    fn draw_line(&mut self, line: [i32; 4], colour: i32, width: i32) -> Result<()>;
    /// A horizontal line (`[x, y, width]`).
    fn draw_horizontal_line(&mut self, line: [i32; 3], colour: i32) -> Result<()>;
    /// A dashed line (`style = [dash, gap, phase]`; `dash <= 0` is a solid
    /// line).
    fn draw_dashed_line(&mut self, line: [i32; 4], colour: i32, style: [i32; 3]) -> Result<()>;
    /// A solid fill through a component graphic's row mask.
    fn fill_masked(&mut self, rect: [i32; 4], colour: i32, mask: &MaskRef) -> Result<()>;
    /// A sprite draw: `colour == -1` is the plain draw (mode 1), anything
    /// else the tint (mode 0).
    fn draw_sprite(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2], colour: i32) -> Result<()>;
    /// A scaled sprite draw into `rect`, tinted by `colour`.
    fn draw_sprite_scaled(
        &mut self,
        sprite: &Rc<Sprite>,
        rect: [i32; 4],
        colour: i32,
    ) -> Result<()>;
    /// A tiled sprite draw over `rect` with `blend`.
    fn draw_sprite_tiled(&mut self, sprite: &Rc<Sprite>, rect: [i32; 4], blend: i32) -> Result<()>;
    /// A graphic component's sprite: tiling, rotation and tint from the
    /// component, its transparency, and the parent bounds to restore after a
    /// tiled draw.
    fn draw_component_sprite(&mut self, draw: &ComponentSprite) -> Result<()>;
    /// Rotate/scale an image about `pivot` (image pixels) placed at `origin`,
    /// optionally through a mask.
    fn draw_rotated(&mut self, draw: &RotatedImage) -> Result<()>;
    /// A parallelogram from three corners `[x0, y0, x1, y1, x2, y2]`,
    /// optionally through a mask.
    fn draw_affine(
        &mut self,
        image: &Image,
        points: [f32; 6],
        colour: i32,
        mask: Option<&MaskRef>,
    ) -> Result<()>;
    /// A sprite drawn through a mask.
    fn draw_sprite_masked(
        &mut self,
        sprite: &Rc<Sprite>,
        pos: [i32; 2],
        mask: &MaskRef,
    ) -> Result<()>;
    /// One font glyph, plain or through a mask.
    /// Only [`Draw::Glyph`] draws are recorded as ops.
    fn draw_glyph(&mut self, font: &Rc<Font>, draw: &Draw, mask: Option<&MaskRef>) -> Result<()>;
}

/// Execute one recorded call on `tk`: the one dispatcher from the
/// display list to a backend.
pub fn draw<T: Toolkit + ?Sized>(tk: &mut T, op: &Op) -> Result<()> {
    match op {
        Op::ResetBounds(bounds) => tk.reset_bounds(*bounds),
        Op::SetBounds(bounds) => tk.set_bounds(*bounds),
        Op::Fill(rect, colour) => tk.fill_rectangle(*rect, *colour),
        Op::Outline(rect, colour) => tk.draw_rectangle(*rect, *colour),
        Op::Line(line, colour, width) => tk.draw_line(*line, *colour, *width),
        Op::HorizontalLine(line, colour) => tk.draw_horizontal_line(*line, *colour),
        Op::DashedLine(line, colour, style) => tk.draw_dashed_line(*line, *colour, *style),
        Op::Sprite(sprite, pos, colour) => tk.draw_sprite(sprite, *pos, *colour),
        Op::Scaled(sprite, rect, colour) => tk.draw_sprite_scaled(sprite, *rect, *colour),
        Op::Tiled(sprite, rect, blend) => tk.draw_sprite_tiled(sprite, *rect, *blend),
        Op::Component(draw) => tk.draw_component_sprite(draw),
        Op::Rotated(draw) => tk.draw_rotated(draw),
        Op::Affine(image, points, colour, mask) => {
            tk.draw_affine(image, *points, *colour, mask.as_ref())
        }
        Op::MaskedFill(rect, colour, mask) => tk.fill_masked(*rect, *colour, mask),
        Op::MaskedSprite(sprite, pos, mask) => tk.draw_sprite_masked(sprite, *pos, mask),
        Op::Glyph(font, draw, mask) => tk.draw_glyph(font, draw, mask.as_ref()),
    }
}

/// [`draw`] every op in order, stopping at the first error.
pub fn replay<T: Toolkit + ?Sized>(tk: &mut T, ops: &[Op]) -> Result<()> {
    ops.iter().try_for_each(|op| draw(tk, op))
}

/// The call name of an op (diagnostics: [`NullToolkit::counts`]).
pub fn op_name(op: &Op) -> &'static str {
    match op {
        Op::ResetBounds(_) => "resetBounds",
        Op::SetBounds(_) => "setBounds",
        Op::Fill(..) => "fillRectangle",
        Op::Outline(..) => "drawRectangle",
        Op::Line(..) => "drawLine",
        Op::HorizontalLine(..) => "drawHorizontalLine",
        Op::DashedLine(..) => "dashedLine",
        Op::Sprite(..) => "drawSprite",
        Op::Scaled(..) => "drawTintedScaled",
        Op::Tiled(..) => "drawTiledTinted",
        Op::Component { .. } => "component sprite",
        Op::Rotated(..) => "rotatedImage",
        Op::Affine(..) => "affineImage",
        Op::MaskedFill(..) => "maskedFill",
        Op::MaskedSprite(..) => "maskedSprite",
        Op::Glyph(..) => "drawChar",
    }
}

/// The GPU backend's recorder: every method is the `Painter` call the UI
/// makes for that draw (recording the [`Op`] and emitting its batch quads).
impl Toolkit for Painter {
    fn reset_bounds(&mut self, bounds: [i32; 4]) -> Result<()> {
        Painter::reset_bounds(self, bounds);
        Ok(())
    }
    fn set_bounds(&mut self, bounds: [i32; 4]) -> Result<()> {
        Painter::set_bounds(self, bounds);
        Ok(())
    }
    fn fill_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()> {
        self.fill(rect, colour)
    }
    fn draw_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()> {
        self.outline(rect, colour);
        Ok(())
    }
    fn draw_line(&mut self, [x0, y0, x1, y1]: [i32; 4], colour: i32, width: i32) -> Result<()> {
        self.line([x0, y0], [x1, y1], colour, width);
        Ok(())
    }
    fn draw_horizontal_line(&mut self, [x, y, width]: [i32; 3], colour: i32) -> Result<()> {
        self.horizontal_line(x, y, width, colour);
        Ok(())
    }
    fn draw_dashed_line(
        &mut self,
        [x0, y0, x1, y1]: [i32; 4],
        colour: i32,
        [dash, gap, phase]: [i32; 3],
    ) -> Result<()> {
        self.dashed_line([x0, y0], [x1, y1], colour, dash, gap, phase);
        Ok(())
    }
    fn fill_masked(&mut self, rect: [i32; 4], colour: i32, mask: &MaskRef) -> Result<()> {
        self.masked_fill(rect, colour, mask.clone());
        Ok(())
    }
    fn draw_sprite(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2], colour: i32) -> Result<()> {
        self.native(sprite, pos, colour);
        Ok(())
    }
    fn draw_sprite_scaled(
        &mut self,
        sprite: &Rc<Sprite>,
        rect: [i32; 4],
        colour: i32,
    ) -> Result<()> {
        self.scaled(sprite, rect, colour)
    }
    fn draw_sprite_tiled(&mut self, sprite: &Rc<Sprite>, rect: [i32; 4], blend: i32) -> Result<()> {
        self.tiled(sprite, rect, blend)
    }
    fn draw_component_sprite(&mut self, draw: &ComponentSprite) -> Result<()> {
        // `Painter::component_sprite` (and `sprite_draw::Painter::component`)
        // read exactly these five component fields.
        let fields = Fields {
            tiling: draw.tiling,
            angle2d: draw.angle2d,
            width: draw.size[0],
            height: draw.size[1],
            colour: draw.colour,
            ..Default::default()
        };
        self.component_sprite(&draw.sprite, &fields, draw.pos, draw.trans, draw.parent)
    }
    fn draw_rotated(&mut self, draw: &RotatedImage) -> Result<()> {
        self.rotated_image(draw.clone());
        Ok(())
    }
    fn draw_affine(
        &mut self,
        image: &Image,
        points: [f32; 6],
        colour: i32,
        mask: Option<&MaskRef>,
    ) -> Result<()> {
        self.affine_image(image.clone(), points, colour, mask.cloned());
        Ok(())
    }
    fn draw_sprite_masked(
        &mut self,
        sprite: &Rc<Sprite>,
        pos: [i32; 2],
        mask: &MaskRef,
    ) -> Result<()> {
        self.masked_native(sprite, pos, mask.clone());
        Ok(())
    }
    fn draw_glyph(&mut self, font: &Rc<Font>, draw: &Draw, mask: Option<&MaskRef>) -> Result<()> {
        self.glyph_masked(font, draw, mask.cloned());
        Ok(())
    }
}

/// The headless backend: records every call it is given (as the [`Op`] it
/// stands for) and draws nothing. Tests replay a frame into it and compare
/// [`NullToolkit::digest`]s; a session replay without a window executes its
/// frames here.
#[derive(Clone, Default)]
pub struct NullToolkit {
    /// Every call, in order.
    pub recording: Recording,
    /// Calls per op kind ([`op_name`]).
    pub counts: BTreeMap<&'static str, usize>,
}

impl NullToolkit {
    pub fn new() -> Self {
        Self::default()
    }
    fn record(&mut self, op: Op) -> Result<()> {
        *self.counts.entry(op_name(&op)).or_default() += 1;
        self.recording.ops.push(op);
        Ok(())
    }
    /// A stable 64-bit FNV-1a hash of the recorded calls: each op's
    /// name, its integer/float arguments bit for bit, and its resources by
    /// content (sprite size, padding and pixels; font metrics and atlas),
    /// not by address, so equal frames hash equal across runs.
    pub fn digest(&self) -> u64 {
        digest(&self.recording.ops)
    }
}

/// [`NullToolkit::digest`] of any op stream.
pub fn digest(ops: &[Op]) -> u64 {
    let mut h = Fnv::default();
    for op in ops {
        describe(&mut h, op);
    }
    h.0
}

impl Toolkit for NullToolkit {
    fn reset_bounds(&mut self, bounds: [i32; 4]) -> Result<()> {
        self.record(Op::ResetBounds(bounds))
    }
    fn set_bounds(&mut self, bounds: [i32; 4]) -> Result<()> {
        self.record(Op::SetBounds(bounds))
    }
    fn fill_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()> {
        self.record(Op::Fill(rect, colour))
    }
    fn draw_rectangle(&mut self, rect: [i32; 4], colour: i32) -> Result<()> {
        self.record(Op::Outline(rect, colour))
    }
    fn draw_line(&mut self, line: [i32; 4], colour: i32, width: i32) -> Result<()> {
        self.record(Op::Line(line, colour, width))
    }
    fn draw_horizontal_line(&mut self, line: [i32; 3], colour: i32) -> Result<()> {
        self.record(Op::HorizontalLine(line, colour))
    }
    fn draw_dashed_line(&mut self, line: [i32; 4], colour: i32, style: [i32; 3]) -> Result<()> {
        self.record(Op::DashedLine(line, colour, style))
    }
    fn fill_masked(&mut self, rect: [i32; 4], colour: i32, mask: &MaskRef) -> Result<()> {
        self.record(Op::MaskedFill(rect, colour, mask.clone()))
    }
    fn draw_sprite(&mut self, sprite: &Rc<Sprite>, pos: [i32; 2], colour: i32) -> Result<()> {
        self.record(Op::Sprite(sprite.clone(), pos, colour))
    }
    fn draw_sprite_scaled(
        &mut self,
        sprite: &Rc<Sprite>,
        rect: [i32; 4],
        colour: i32,
    ) -> Result<()> {
        self.record(Op::Scaled(sprite.clone(), rect, colour))
    }
    fn draw_sprite_tiled(&mut self, sprite: &Rc<Sprite>, rect: [i32; 4], blend: i32) -> Result<()> {
        self.record(Op::Tiled(sprite.clone(), rect, blend))
    }
    fn draw_component_sprite(&mut self, draw: &ComponentSprite) -> Result<()> {
        self.record(Op::Component(draw.clone()))
    }
    fn draw_rotated(&mut self, draw: &RotatedImage) -> Result<()> {
        self.record(Op::Rotated(draw.clone()))
    }
    fn draw_affine(
        &mut self,
        image: &Image,
        points: [f32; 6],
        colour: i32,
        mask: Option<&MaskRef>,
    ) -> Result<()> {
        self.record(Op::Affine(image.clone(), points, colour, mask.cloned()))
    }
    fn draw_sprite_masked(
        &mut self,
        sprite: &Rc<Sprite>,
        pos: [i32; 2],
        mask: &MaskRef,
    ) -> Result<()> {
        self.record(Op::MaskedSprite(sprite.clone(), pos, mask.clone()))
    }
    fn draw_glyph(&mut self, font: &Rc<Font>, draw: &Draw, mask: Option<&MaskRef>) -> Result<()> {
        self.record(Op::Glyph(font.clone(), draw.clone(), mask.cloned()))
    }
}

/// 64-bit FNV-1a.
struct Fnv(u64);
impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}
impl Fnv {
    fn bytes(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    fn ints(&mut self, v: &[i32]) {
        v.iter().for_each(|v| self.bytes(&v.to_le_bytes()));
    }
    fn floats(&mut self, v: &[f32]) {
        v.iter()
            .for_each(|v| self.bytes(&v.to_bits().to_le_bytes()));
    }
    fn sprite(&mut self, s: &Sprite) {
        self.ints(&s.size);
        self.ints(&s.padding);
        self.ints(&s.argb);
    }
    fn font(&mut self, f: &Font) {
        self.bytes(format!("{:?}", f.metrics).as_bytes());
        self.bytes(&[f.atlas.scale]);
        self.ints(&[i32::from(f.atlas.width), i32::from(f.atlas.height)]);
        f.atlas
            .argb
            .iter()
            .for_each(|v| self.bytes(&v.to_le_bytes()));
        self.bytes(&[
            u8::from(f.monochrome),
            u8::from(f.paletted),
            u8::from(f.translucent),
        ]);
    }
    fn image(&mut self, image: &Image) {
        match image {
            Image::Sprite(s) => {
                self.bytes(b"S");
                self.sprite(s);
            }
            Image::Font(f) => {
                self.bytes(b"F");
                self.font(f);
            }
            Image::White => self.bytes(b"W"),
            Image::External(id) => {
                self.bytes(b"E");
                self.bytes(&id.to_le_bytes());
            }
        }
    }
    fn mask(&mut self, mask: Option<&MaskRef>) {
        match mask {
            Some(m) => {
                self.bytes(b"M");
                self.sprite(&m.sprite);
                self.ints(&m.origin);
            }
            None => self.bytes(b"-"),
        }
    }
}

fn describe(h: &mut Fnv, op: &Op) {
    h.bytes(op_name(op).as_bytes());
    match op {
        Op::ResetBounds(v) | Op::SetBounds(v) => h.ints(v),
        Op::Fill(r, c) | Op::Outline(r, c) => {
            h.ints(r);
            h.ints(&[*c]);
        }
        Op::Line(l, c, w) => {
            h.ints(l);
            h.ints(&[*c, *w]);
        }
        Op::HorizontalLine(l, c) => {
            h.ints(l);
            h.ints(&[*c]);
        }
        Op::DashedLine(l, c, s) => {
            h.ints(l);
            h.ints(&[*c]);
            h.ints(s);
        }
        Op::Sprite(s, p, c) => {
            h.sprite(s);
            h.ints(p);
            h.ints(&[*c]);
        }
        Op::Scaled(s, r, c) | Op::Tiled(s, r, c) => {
            h.sprite(s);
            h.ints(r);
            h.ints(&[*c]);
        }
        Op::Component(c) => {
            h.sprite(&c.sprite);
            h.ints(&[i32::from(c.tiling), c.angle2d, c.colour, c.trans]);
            h.ints(&c.size);
            h.ints(&c.pos);
            h.ints(&c.parent);
        }
        Op::Rotated(r) => {
            h.image(&r.image);
            h.ints(&r.size);
            h.floats(&r.origin);
            h.floats(&r.pivot);
            h.ints(&[r.scale, r.angle, r.colour]);
            h.mask(r.mask.as_ref());
        }
        Op::Affine(image, points, colour, mask) => {
            h.image(image);
            h.floats(points);
            h.ints(&[*colour]);
            h.mask(mask.as_ref());
        }
        Op::MaskedFill(r, c, mask) => {
            h.ints(r);
            h.ints(&[*c]);
            h.mask(Some(mask));
        }
        Op::MaskedSprite(s, p, mask) => {
            h.sprite(s);
            h.ints(p);
            h.mask(Some(mask));
        }
        Op::Glyph(font, draw, mask) => {
            h.font(font);
            h.bytes(format!("{draw:?}").as_bytes());
            h.mask(mask.as_ref());
        }
    }
}

#[cfg(test)]
#[path = "toolkit_tests.rs"]
mod tests;
