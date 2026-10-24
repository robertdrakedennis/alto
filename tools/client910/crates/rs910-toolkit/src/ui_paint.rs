//! Shared-target 2D painter. Text glyphs and sprites feed the same batch
//! shader in traversal order; primitives use the painter's white sprite.
use crate::{
    font::Font,
    font_layout::Draw,
    sprite::Sprite,
    sprite_draw,
    text_render::{self, Vertex},
};
use anyhow::Result;
use std::rc::{Rc, Weak};
#[derive(Clone)]
pub enum Image {
    Sprite(Rc<Sprite>),
    Font(Rc<Font>),
    White,
    /// A render target registered with [`Renderer::register_external`]
    /// (the minimap base sprite).
    External(u64),
}
impl Image {
    pub fn key(&self) -> (u8, usize) {
        match self {
            Self::Sprite(v) => (0, Rc::as_ptr(v) as usize),
            Self::Font(v) => (1, Rc::as_ptr(v) as usize),
            Self::White => (2, 0),
            Self::External(id) => (3, *id as usize),
        }
    }
    pub fn weak(&self) -> Owner {
        match self {
            Self::Sprite(v) => Owner::Sprite(Rc::downgrade(v)),
            Self::Font(v) => Owner::Font(Rc::downgrade(v)),
            Self::White => Owner::White,
            Self::External(_) => Owner::White,
        }
    }
}
pub enum Owner {
    Sprite(Weak<Sprite>),
    Font(Weak<Font>),
    White,
}
impl Owner {
    pub fn alive(&self) -> bool {
        match self {
            Self::Sprite(v) => v.strong_count() != 0,
            Self::Font(v) => v.strong_count() != 0,
            Self::White => true,
        }
    }
}
/// A draw mask: the component graphic's sprite, positioned at the
/// component origin.
#[derive(Clone)]
pub struct MaskRef {
    pub sprite: Rc<Sprite>,
    pub origin: [i32; 2],
}
impl MaskRef {
    pub fn key(&self) -> (usize, [i32; 2]) {
        (Rc::as_ptr(&self.sprite) as usize, self.origin)
    }
}
#[derive(Clone)]
pub struct Quad {
    pub image: Image,
    pub vertices: [Vertex; 4],
    pub clip: [i32; 4],
    pub mask: Option<MaskRef>,
}
pub struct Plan {
    pub size: [u32; 2],
    pub quads: Vec<Quad>,
}

/// Convert canvas bounds to framebuffer edges. Geometry is normalized
/// by the canvas, so use the actual target/canvas ratio, including rounding
/// of a HiDPI window's logical size. Shared edges must round identically.
pub fn framebuffer_bounds(bounds: [i32; 4], canvas: [u32; 2], target: [u32; 2]) -> [i32; 4] {
    std::array::from_fn(|i| {
        let axis = i % 2;
        let extent = i64::from(canvas[axis]);
        let edge = i64::from(bounds[i]).clamp(0, extent);
        ((edge * i64::from(target[axis]) + extent / 2) / extent) as i32
    })
}
impl From<sprite_draw::Painter> for Plan {
    fn from(v: sprite_draw::Painter) -> Self {
        Self {
            size: v.size,
            quads: v
                .quads
                .into_iter()
                .map(|q| Quad {
                    image: Image::Sprite(q.sprite),
                    vertices: q.vertices,
                    clip: q.clip,
                    mask: None,
                })
                .collect(),
        }
    }
}
/// A graphic component's sprite draw: tiling, rotation and tint come from the
/// component, `trans` is its transparency and `parent` the bounds restored
/// after a tiled draw.
#[derive(Clone)]
pub struct ComponentSprite {
    pub sprite: Rc<Sprite>,
    pub tiling: bool,
    pub angle2d: i32,
    pub size: [i32; 2],
    pub colour: i32,
    pub pos: [i32; 2],
    pub trans: i32,
    pub parent: [i32; 4],
}
/// A rotated and scaled image draw: rotate/scale about `pivot` (image pixels)
/// placed at `origin`, `angle` in 1/65536 turns, `scale` in 1/4096, optionally
/// through a mask.
#[derive(Clone)]
pub struct RotatedImage {
    pub image: Image,
    pub size: [i32; 2],
    pub origin: [f32; 2],
    pub pivot: [f32; 2],
    pub scale: i32,
    pub angle: i32,
    pub colour: i32,
    pub mask: Option<MaskRef>,
}
/// One toolkit/sprite/font call behind a painter primitive, in traversal
/// order. The GPU path consumes [`Painter::quads`]; the headless
/// [`crate::toolkit::NullToolkit`] replays and digests these. Colours and
/// modes are the arguments the primitive stands for.
#[derive(Clone)]
pub enum Op {
    /// Reset the clip to `[x0, y0, x1, y1]`.
    ResetBounds([i32; 4]),
    /// Intersect the clip with `[x0, y0, x1, y1]`.
    SetBounds([i32; 4]),
    /// Fill `[x, y, w, h]` (blend 1).
    Fill([i32; 4], i32),
    /// Outline `[x, y, w, h]` (blend 1).
    Outline([i32; 4], i32),
    /// A line `[x0, y0, x1, y1]` with `colour` and optional width.
    Line([i32; 4], i32, i32),
    /// A horizontal line `[x, y, w]` (font decorations).
    HorizontalLine([i32; 3], i32),
    /// A styled (dashed) polygon edge, or a plain line when `dash <= 0`.
    DashedLine([i32; 4], i32, [i32; 3]),
    /// A sprite draw: colour `-1` is the plain mode.
    Sprite(Rc<Sprite>, [i32; 2], i32),
    /// A scaled, tinted sprite draw into `[x, y, w, h]`.
    Scaled(Rc<Sprite>, [i32; 4], i32),
    /// A tiled sprite draw (a plain tiled draw uses blend 1).
    Tiled(Rc<Sprite>, [i32; 4], i32),
    /// A graphic component's sprite (type-5 component draw).
    Component(ComponentSprite),
    /// Rotate/scale about a pivot.
    Rotated(RotatedImage),
    /// A parallelogram from three corners.
    Affine(Image, [f32; 6], i32, Option<MaskRef>),
    /// A solid fill through a component graphic's row mask.
    MaskedFill([i32; 4], i32, MaskRef),
    /// A sprite drawn through a mask.
    MaskedSprite(Rc<Sprite>, [i32; 2], MaskRef),
    /// One font glyph, plain or through a mask.
    Glyph(Rc<Font>, Draw, Option<MaskRef>),
}
/// The ops of one painter and, per [`Painter::quad_count`] call, the
/// `(quads, ops)` lengths at that point: GPU split boundaries map onto op
/// indices through these marks.
#[derive(Clone, Default)]
pub struct Recording {
    pub ops: Vec<Op>,
    pub marks: Vec<(usize, usize)>,
}
impl Recording {
    /// The op index matching a quad boundary (the last mark recorded at
    /// that quad count; the op total when none was recorded).
    pub fn op_index(&self, quad: usize) -> usize {
        self.marks
            .iter()
            .rev()
            .find(|(q, _)| *q == quad)
            .map_or(self.ops.len(), |(_, o)| *o)
    }
}
pub struct Painter {
    pub sprite: sprite_draw::Painter,
    pub quads: Vec<Quad>,
    white: Rc<Sprite>,
    pub recording: Recording,
    /// Offscreen `FrameBuffer` sprites this painter draws, by
    /// `Image::External` id (rendered before `quads`).
    pub layers: Vec<(u64, Plan)>,
}
impl Painter {
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            sprite: sprite_draw::Painter::new(size),
            quads: vec![],
            white: Rc::new(Sprite {
                paletted: None,
                size: [1, 1],
                padding: [0; 4],
                argb: vec![-1],
            }),
            recording: Recording::default(),
            layers: Vec::new(),
        }
    }
    /// A framebuffer sprite: a canvas-sized sprite bound as the colour target
    /// while `layer` drew, then drawn at the origin with `colour`. The GPU
    /// renders `layer` into target `id` and composites it here with
    /// `colour`'s alpha; the layer's calls are appended to this recording.
    pub fn framebuffer_sprite(&mut self, id: u64, layer: Painter, colour: i32) {
        let mut layer = layer;
        self.recording.ops.append(&mut layer.recording.ops);
        self.layers.append(&mut layer.layers);
        self.layers.push((id, layer.finish()));
        let [w, h] = self.sprite.size.map(|v| v as f32);
        self.affine_quad(
            Image::External(id),
            [0.0, 0.0, w, 0.0, 0.0, h],
            colour,
            None,
        );
    }
    fn record(&mut self, op: Op) {
        self.recording.ops.push(op);
    }
    pub fn reset_bounds(&mut self, c: [i32; 4]) {
        self.record(Op::ResetBounds(c));
        self.sprite.reset_bounds(c);
    }
    pub fn set_bounds(&mut self, c: [i32; 4]) {
        self.record(Op::SetBounds(c));
        self.sprite.set_bounds(c);
    }
    fn flush_sprites(&mut self) {
        self.quads.extend(self.sprite.quads.drain(..).map(|q| Quad {
            image: if Rc::ptr_eq(&q.sprite, &self.white) {
                Image::White
            } else {
                Image::Sprite(q.sprite)
            },
            vertices: q.vertices,
            clip: q.clip,
            mask: None,
        }));
    }
    /// `sprite_draw::Painter::scaled` through the shared painter.
    pub fn scaled(&mut self, s: &Rc<Sprite>, rect: [i32; 4], colour: i32) -> Result<()> {
        self.record(Op::Scaled(s.clone(), rect, colour));
        let result = self.sprite.scaled(s, rect, colour);
        self.flush_sprites();
        result
    }
    /// Batch-quad geometry for any image (see `sprite_draw::Painter::affine`),
    /// optionally through a mask: a parallelogram from three corner points.
    pub fn affine_image(
        &mut self,
        image: Image,
        points: [f32; 6],
        colour: i32,
        mask: Option<MaskRef>,
    ) {
        self.record(Op::Affine(image.clone(), points, colour, mask.clone()));
        self.affine_quad(image, points, colour, mask);
    }
    fn affine_quad(&mut self, image: Image, points: [f32; 6], colour: i32, mask: Option<MaskRef>) {
        if colour as u32 >> 24 == 0 {
            return;
        }
        self.flush_sprites();
        let [x0, y0, x1, y1, x2, y2] = points;
        let p = [[x0, y0], [x1, y1], [x2, y2], [x1 + x2 - x0, y1 + y2 - y0]];
        let clip = self.sprite.clip;
        let c = clip.map(|v| v as f32);
        if p.iter().all(|v| v[0] < c[0])
            || p.iter().all(|v| v[0] > c[2])
            || p.iter().all(|v| v[1] < c[1])
            || p.iter().all(|v| v[1] > c[3])
        {
            return;
        }
        let col = colour as u32;
        let colour = [
            (col >> 16) as u8,
            (col >> 8) as u8,
            col as u8,
            (col >> 24) as u8,
        ];
        let size = self.sprite.size;
        let uv = [[0., 0.], [1., 0.], [0., 1.], [1., 1.]];
        let vertices = std::array::from_fn(|i| Vertex {
            position: [
                p[i][0] / size[0] as f32 * 2. - 1.,
                (1. - p[i][1] / size[1] as f32) * 2. - 1.,
            ],
            uv: uv[i],
            colour,
        });
        self.quads.push(Quad {
            image,
            vertices,
            clip,
            mask,
        });
    }
    /// Rotate/scale about `pivot` (image
    /// pixels) placed at `origin`, `angle` in 1/65536 turns, `scale` in 1/4096.
    pub fn rotated_image(&mut self, rotated: RotatedImage) {
        self.record(Op::Rotated(rotated.clone()));
        let RotatedImage {
            image,
            size,
            origin,
            pivot,
            scale,
            angle,
            colour,
            mask,
        } = rotated;
        if scale == 0 {
            return;
        }
        let [sin, cos] = sprite_draw::trig(angle);
        let s = sin * scale as f32;
        let c = cos * scale as f32;
        let [x, y] = pivot;
        let [w, h] = size.map(|v| v as f32);
        let [ox, oy] = origin;
        self.affine_quad(
            image,
            [
                (-x * c + -y * s) / 4096. + ox,
                (x * s + -y * c) / 4096. + oy,
                ((w - x) * c + -y * s) / 4096. + ox,
                (-(w - x) * s + -y * c) / 4096. + oy,
                (-x * c + (h - y) * s) / 4096. + ox,
                (x * s + (h - y) * c) / 4096. + oy,
            ],
            colour,
            mask,
        );
    }
    /// A solid fill through the mask.
    pub fn masked_fill(&mut self, rect: [i32; 4], colour: i32, mask: MaskRef) {
        self.record(Op::MaskedFill(rect, colour, mask.clone()));
        let [x, y, w, h] = rect.map(|v| v as f32);
        self.affine_quad(Image::White, [x, y, x + w, y, x, y + h], colour, Some(mask));
    }
    pub fn quad_count(&mut self) -> usize {
        self.flush_sprites();
        let mark = (self.quads.len(), self.recording.ops.len());
        self.recording.marks.push(mark);
        self.quads.len()
    }
    /// A tiled sprite draw: the device path
    /// tiles `sprite_draw::Painter::tiled` untinted.
    pub fn tiled(&mut self, s: &Rc<Sprite>, rect: [i32; 4], blend: i32) -> Result<()> {
        self.record(Op::Tiled(s.clone(), rect, blend));
        let result = self.sprite.tiled(s, rect, -1);
        self.flush_sprites();
        result
    }
    pub fn native(&mut self, s: &Rc<Sprite>, pos: [i32; 2], colour: i32) {
        self.record(Op::Sprite(s.clone(), pos, colour));
        self.sprite.sprite(s, pos, colour);
        self.flush_sprites();
    }
    /// The unscaled sprite at its padding offset, through the mask.
    pub fn masked_native(&mut self, s: &Rc<Sprite>, pos: [i32; 2], mask: MaskRef) {
        self.record(Op::MaskedSprite(s.clone(), pos, mask.clone()));
        self.masked_native_quad(s, pos, -1, mask);
    }
    /// A font `<img>`/`<sprite>` tag drawn by a masked text component. The
    /// image is drawn with the plain sprite draw, ignoring the mask.
    pub fn masked_native_tinted(
        &mut self,
        s: &Rc<Sprite>,
        pos: [i32; 2],
        colour: i32,
        mask: MaskRef,
    ) {
        self.record(Op::Sprite(s.clone(), pos, colour));
        self.masked_native_quad(s, pos, colour, mask);
    }
    fn masked_native_quad(&mut self, s: &Rc<Sprite>, pos: [i32; 2], colour: i32, mask: MaskRef) {
        let x = pos[0].wrapping_add(s.padding[0]) as f32;
        let y = pos[1].wrapping_add(s.padding[1]) as f32;
        let [w, h] = s.size.map(|v| v as f32);
        self.affine_quad(
            Image::Sprite(s.clone()),
            [x, y, x + w, y, x, y + h],
            colour,
            Some(mask),
        );
    }
    pub fn component_sprite(
        &mut self,
        s: &Rc<Sprite>,
        f: &crate::ui_component_fields::Fields,
        pos: [i32; 2],
        trans: i32,
        parent: [i32; 4],
    ) -> Result<()> {
        self.record(Op::Component(ComponentSprite {
            sprite: s.clone(),
            tiling: f.tiling,
            angle2d: f.angle2d,
            size: [f.width, f.height],
            colour: f.colour,
            pos,
            trans,
            parent,
        }));
        let result = self.sprite.component(s, f, pos, trans, parent);
        self.flush_sprites();
        result
    }
    pub fn glyph(&mut self, font: &Rc<Font>, draw: &Draw) {
        self.glyph_masked(font, draw, None);
    }
    pub fn glyph_masked(&mut self, font: &Rc<Font>, draw: &Draw, mask: Option<MaskRef>) {
        if matches!(draw, Draw::Glyph { .. }) {
            self.record(Op::Glyph(font.clone(), draw.clone(), mask.clone()));
        }
        if let Some(vertices) =
            text_render::glyph(&font.metrics, draw, self.sprite.clip, self.sprite.size)
        {
            self.quads.push(Quad {
                image: Image::Font(font.clone()),
                vertices,
                clip: self.sprite.clip,
                mask,
            });
        }
    }
    /// A filled rectangle. In the default batch mode its
    /// white sprite follows the same tint/alpha behavior regardless of blend.
    pub fn fill(&mut self, rect: [i32; 4], colour: i32) -> Result<()> {
        self.record(Op::Fill(rect, colour));
        let result = self.sprite.scaled(&self.white, rect, colour);
        self.flush_sprites();
        result
    }
    /// A line. Preserve float->int casts, endpoint
    /// extension, normalization and thickness before the affine sprite call.
    pub fn line(&mut self, from: [i32; 2], to: [i32; 2], colour: i32, width: i32) {
        self.record(Op::Line([from[0], from[1], to[0], to[1]], colour, width));
        self.line_quad(from, to, colour, width);
    }
    /// A font strikethrough/underline (a horizontal line), drawn on the GPU
    /// as a line.
    pub fn horizontal_line(&mut self, x: i32, y: i32, width: i32, colour: i32) {
        self.record(Op::HorizontalLine([x, y, width], colour));
        self.line_quad([x, y], [x.wrapping_add(width), y], colour, 1);
    }
    fn line_quad(&mut self, from: [i32; 2], to: [i32; 2], colour: i32, width: i32) {
        let a = from.map(|v| (v as f32 + 1.) as i32);
        let b = to.map(|v| (v as f32 + 1.) as i32);
        let dx = b[0].wrapping_sub(a[0]) as f32;
        let dy = b[1].wrapping_sub(a[1]) as f32;
        let inv = 1. / ((dx * dx + dy * dy) as f64).sqrt() as f32;
        let nx = dx * inv;
        let ny = dy * inv;
        let start = [(a[0] as f32 - nx) as i32, (a[1] as f32 - ny) as i32];
        let ox = width as f32 * 0.5 * -ny;
        let oy = width as f32 * 0.5 * nx;
        self.sprite.affine(
            &self.white,
            [
                start[0] as f32 - ox,
                start[1] as f32 - oy,
                b[0] as f32 - ox,
                b[1] as f32 - oy,
                start[0] as f32 + ox,
                start[1] as f32 + oy,
            ],
            colour,
        );
        self.flush_sprites();
    }
    /// Direct one-pixel segment used by the dashed-line primitive. Unlike
    /// `drawLine`, the dashed primitive does not extend either endpoint.
    fn segment(&mut self, from: [f32; 2], to: [f32; 2], colour: i32) {
        let dx = to[0] - from[0];
        let dy = to[1] - from[1];
        let length = dx.hypot(dy);
        if length == 0.0 {
            let _ = self.sprite.scaled(
                &self.white.clone(),
                [from[0] as i32, from[1] as i32, 1, 1],
                colour,
            );
            self.flush_sprites();
            return;
        }
        let ox = dy / length * 0.5;
        let oy = -dx / length * 0.5;
        self.sprite.affine(
            &self.white,
            [
                from[0] - ox,
                from[1] - oy,
                to[0] - ox,
                to[1] - oy,
                from[0] + ox,
                from[1] + oy,
            ],
            colour,
        );
        self.flush_sprites();
    }
    /// Styled polygon edge: a plain one-pixel line for a solid style, else
    /// the dashed-line primitive over the canonicalised edge
    /// (`crate::line_dashes::dashes`), each dash a direct one-pixel
    /// segment without the `drawLine` endpoint extension.
    pub fn dashed_line(
        &mut self,
        from: [i32; 2],
        to: [i32; 2],
        colour: i32,
        dash: i32,
        gap: i32,
        phase: i32,
    ) {
        self.record(Op::DashedLine(
            [from[0], from[1], to[0], to[1]],
            colour,
            [dash, gap, phase],
        ));
        if dash <= 0 {
            self.line_quad(from, to, colour, 1);
            return;
        }
        let (from, to) = crate::line_dashes::edge_order(from, to);
        let right = self.sprite.clip[2];
        for [x0, y0, x1, y1] in crate::line_dashes::dashes(from, to, dash, gap, phase, right) {
            self.segment([x0, y0], [x1, y1], colour);
        }
    }
    /// A rectangle outline, including the GLX horizontal shift.
    pub fn outline(&mut self, [x, y, w, h]: [i32; 4], colour: i32) {
        self.record(Op::Outline([x, y, w, h], colour));
        let right = x.wrapping_add(w.wrapping_sub(1));
        let bottom = y.wrapping_add(h.wrapping_sub(1));
        self.line_quad(
            [x, y.wrapping_sub(1)],
            [right, y.wrapping_sub(1)],
            colour,
            1,
        );
        self.line_quad(
            [x, bottom.wrapping_sub(1)],
            [right, bottom.wrapping_sub(1)],
            colour,
            1,
        );
        self.line_quad([x, y], [x, bottom], colour, 1);
        self.line_quad([right, y], [right, bottom], colour, 1);
    }
    pub fn finish(mut self) -> Plan {
        self.flush_sprites();
        Plan {
            size: self.sprite.size,
            quads: self.quads,
        }
    }
}
