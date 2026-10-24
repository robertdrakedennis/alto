//! The default sprite batch path, including its distinct padding treatment
//! for scaling and rotation. Immediate/masked sprite-shader
//! calls remain a separate graphics consumer, not silently routed here.
use crate::{
    sprite::Sprite,
    text_render::{self, Vertex},
    ui_component_fields::Fields,
};
use anyhow::Result;
use std::rc::Rc;
#[derive(Clone, Debug)]
pub struct Quad {
    pub sprite: Rc<Sprite>,
    pub vertices: [Vertex; 4],
    pub clip: [i32; 4],
}
#[derive(Default)]
pub struct Painter {
    pub size: [u32; 2],
    pub clip: [i32; 4],
    pub quads: Vec<Quad>,
}
impl Painter {
    pub fn new(size: [u32; 2]) -> Self {
        Self {
            size,
            clip: [0, 0, size[0] as i32, size[1] as i32],
            quads: vec![],
        }
    }
    /// Resetting the clip has an inclusive full-target shortcut.
    pub fn reset_bounds(&mut self, c: [i32; 4]) {
        let [w, h] = self.size.map(|v| v as i32);
        self.clip =
            if c[0] <= 0 && c[2] >= w.wrapping_sub(1) && c[1] <= 0 && c[3] >= h.wrapping_sub(1) {
                [0, 0, w, h]
            } else {
                [c[0].max(0), c[1].max(0), c[2].min(w), c[3].min(h)]
            };
    }
    /// Setting the clip intersects the currently retained bounds.
    pub fn set_bounds(&mut self, c: [i32; 4]) {
        self.clip = [
            self.clip[0].max(c[0].max(0)),
            self.clip[1].max(c[1].max(0)),
            self.clip[2].min(c[2].min(self.size[0] as i32)),
            self.clip[3].min(c[3].min(self.size[1] as i32)),
        ];
    }
    fn axis(&mut self, s: &Rc<Sprite>, rect: [f32; 4], uv: [f32; 4], colour: i32) {
        if let Some(vertices) = text_render::quad(rect, uv, colour as u32, self.clip, self.size) {
            self.quads.push(Quad {
                sprite: s.clone(),
                vertices,
                clip: self.clip,
            });
        }
    }
    pub fn sprite(&mut self, s: &Rc<Sprite>, pos: [i32; 2], colour: i32) {
        let x = pos[0].wrapping_add(s.padding[0]);
        let y = pos[1].wrapping_add(s.padding[1]);
        self.axis(
            s,
            [
                x as f32,
                y as f32,
                x.wrapping_add(s.size[0]) as f32,
                y.wrapping_add(s.size[1]) as f32,
            ],
            [0., 0., 1., 1.],
            colour,
        );
    }
    pub fn scaled(&mut self, s: &Rc<Sprite>, mut rect: [i32; 4], colour: i32) -> Result<()> {
        if s.padding.iter().any(|v| *v != 0) {
            let full = s.full_size();
            rect[2] = div(s.size[0].wrapping_mul(rect[2]), full[0])?;
            rect[3] = div(s.size[1].wrapping_mul(rect[3]), full[1])?;
            rect[0] = rect[0].wrapping_add(div(s.padding[0].wrapping_mul(rect[2]), s.size[0])?);
            rect[1] = rect[1].wrapping_add(div(s.padding[1].wrapping_mul(rect[3]), s.size[1])?);
        }
        self.axis(
            s,
            [
                rect[0] as f32,
                rect[1] as f32,
                rect[0].wrapping_add(rect[2]) as f32,
                rect[1].wrapping_add(rect[3]) as f32,
            ],
            [0., 0., 1., 1.],
            colour,
        );
        Ok(())
    }
    pub fn tiled(&mut self, s: &Rc<Sprite>, rect: [i32; 4], colour: i32) -> Result<()> {
        let end_y = rect[1].wrapping_add(rect[3]);
        let end_x = rect[0].wrapping_add(rect[2]);
        let full = s.full_size();
        let mut y = s.padding[1].wrapping_add(rect[1]);
        let mut bottom = s.size[1].wrapping_add(y);
        // TODO(#gap-H-divergent-sprite-inputs): refuse a non-positive tile
        // period when the original would enter a non-terminating render loop.
        if bottom <= end_y {
            anyhow::ensure!(full[1] > 0, "non-progressing sprite tile period");
        }
        while bottom <= end_y {
            let mut x = s.padding[0].wrapping_add(rect[0]);
            let mut right = s.size[0].wrapping_add(x);
            if right <= end_x {
                anyhow::ensure!(full[0] > 0, "non-progressing sprite tile period");
            }
            while right <= end_x {
                self.axis(
                    s,
                    [
                        x as f32,
                        y as f32,
                        x.wrapping_add(s.size[0]) as f32,
                        y.wrapping_add(s.size[1]) as f32,
                    ],
                    [0., 0., 1., 1.],
                    colour,
                );
                x = x.wrapping_add(full[0]);
                right = right.wrapping_add(full[0]);
            }
            if x < end_x {
                let w = end_x.wrapping_sub(x);
                self.axis(
                    s,
                    [
                        x as f32,
                        y as f32,
                        x.wrapping_add(w) as f32,
                        y.wrapping_add(s.size[1]) as f32,
                    ],
                    [0., 0., w as f32 / s.size[0] as f32 * 1., 1.],
                    colour,
                );
            }
            y = y.wrapping_add(full[1]);
            bottom = bottom.wrapping_add(full[1]);
        }
        if y < end_y {
            let h = end_y.wrapping_sub(y);
            let v = h as f32 / s.size[1] as f32 * 1.;
            let mut x = s.padding[0].wrapping_add(rect[0]);
            let mut right = s.size[0].wrapping_add(x);
            if right <= end_x {
                anyhow::ensure!(full[0] > 0, "non-progressing sprite tile period");
            }
            while right <= end_x {
                self.axis(
                    s,
                    [
                        x as f32,
                        y as f32,
                        x.wrapping_add(s.size[0]) as f32,
                        y.wrapping_add(h) as f32,
                    ],
                    [0., 0., 1., v],
                    colour,
                );
                x = x.wrapping_add(full[0]);
                right = right.wrapping_add(full[0]);
            }
            if x < end_x {
                let w = end_x.wrapping_sub(x);
                self.axis(
                    s,
                    [
                        x as f32,
                        y as f32,
                        x.wrapping_add(w) as f32,
                        y.wrapping_add(h) as f32,
                    ],
                    [0., 0., w as f32 / s.size[0] as f32 * 1., v],
                    colour,
                );
            }
        }
        Ok(())
    }
    /// Reject a quad wholly outside one side;
    /// retain its vertices and let the hardware scissor clip a straddling quad.
    pub fn affine(&mut self, s: &Rc<Sprite>, points: [f32; 6], colour: i32) {
        if colour as u32 >> 24 == 0 {
            return;
        }
        let [x0, y0, x1, y1, x2, y2] = points;
        let p = [[x0, y0], [x1, y1], [x2, y2], [x1 + x2 - x0, y1 + y2 - y0]];
        let c = self.clip.map(|v| v as f32);
        if p.iter().all(|v| v[0] < c[0])
            || p.iter().all(|v| v[0] > c[2])
            || p.iter().all(|v| v[1] < c[1])
            || p.iter().all(|v| v[1] > c[3])
        {
            return;
        }
        // The saved bounds are re-applied before
        // flushing a straddling affine quad. resetBounds can expand a clip
        // ending at width-1/height-1 to the full target; that state persists.
        if p.iter()
            .any(|v| v[0] < c[0] || v[0] > c[2] || v[1] < c[1] || v[1] > c[3])
        {
            self.reset_bounds(self.clip);
        }
        let col = colour as u32;
        let colour = [
            (col >> 16) as u8,
            (col >> 8) as u8,
            col as u8,
            (col >> 24) as u8,
        ];
        let uv = [[0., 0.], [1., 0.], [0., 1.], [1., 1.]];
        let vertices = std::array::from_fn(|i| Vertex {
            position: [
                p[i][0] / self.size[0] as f32 * 2. - 1.,
                (1. - p[i][1] / self.size[1] as f32) * 2. - 1.,
            ],
            uv: uv[i],
            colour,
        });
        self.quads.push(Quad {
            sprite: s.clone(),
            vertices,
            clip: self.clip,
        });
    }
    pub fn rotated(
        &mut self,
        s: &Rc<Sprite>,
        origin: [f32; 2],
        pivot: [f32; 2],
        scale: [i32; 2],
        angle: i32,
        colour: i32,
    ) {
        if scale[0] == 0 || scale[1] == 0 {
            return;
        }
        let [sin, cos] = trig(angle);
        let sy = sin * scale[1] as f32;
        let cy = cos * scale[1] as f32;
        let sx = sin * scale[0] as f32;
        let cx = cos * scale[0] as f32;
        let [x, y] = pivot;
        let [w, h] = s.full_size().map(|v| v as f32);
        let [ox, oy] = origin;
        self.affine(
            s,
            [
                (-x * cx + -y * sy) / 4096. + ox,
                (x * sx + -y * cy) / 4096. + oy,
                ((w - x) * cx + -y * sy) / 4096. + ox,
                (-(w - x) * sx + -y * cy) / 4096. + oy,
                (-x * cx + (h - y) * sy) / 4096. + ox,
                (x * sx + (h - y) * cy) / 4096. + oy,
            ],
            colour,
        );
    }
    /// A graphic component's sprite draw, after resolving the sprite. This
    /// preserves the tint-dependent choice of uniform versus two-axis scale.
    pub fn component(
        &mut self,
        s: &Rc<Sprite>,
        f: &Fields,
        pos: [i32; 2],
        trans: i32,
        parent: [i32; 4],
    ) -> Result<()> {
        let [w, h] = s.full_size();
        let alpha = 255 - (trans & 255);
        if alpha == 0 {
            return Ok(());
        }
        let rgb = if f.colour == -1 || f.colour & 0xffffff == 0 {
            0xffffff
        } else {
            f.colour & 0xffffff
        };
        let colour = rgb | (alpha << 24);
        let tinted = colour != -1;
        let [x, y] = pos;
        if f.tiling {
            self.set_bounds([x, y, x.wrapping_add(f.width), y.wrapping_add(f.height)]);
            if f.angle2d != 0 {
                let cols = div(f.width.wrapping_add(w.wrapping_sub(1)), w)?;
                let rows = div(f.height.wrapping_add(h.wrapping_sub(1)), h)?;
                for col in 0..cols {
                    for row in 0..rows {
                        self.rotated(
                            s,
                            [
                                w as f32 / 2. + w.wrapping_mul(col).wrapping_add(x) as f32,
                                h as f32 / 2. + h.wrapping_mul(row).wrapping_add(y) as f32,
                            ],
                            [w as f32 / 2., h as f32 / 2.],
                            [4096, 4096],
                            f.angle2d,
                            colour,
                        );
                    }
                }
            } else {
                self.tiled(s, [x, y, f.width, f.height], colour)?;
            }
            self.reset_bounds(parent);
        } else if f.angle2d != 0 {
            let sx = div(f.width.wrapping_mul(4096), w)?;
            let sy = if tinted {
                sx
            } else {
                div(f.height.wrapping_mul(4096), h)?
            };
            self.rotated(
                s,
                [
                    f.width as f32 / 2. + x as f32,
                    f.height as f32 / 2. + y as f32,
                ],
                [w as f32 / 2., h as f32 / 2.],
                [sx, sy],
                f.angle2d,
                colour,
            );
        } else if f.width == w && f.height == h {
            self.sprite(s, pos, colour);
        } else {
            self.scaled(s, [x, y, f.width, f.height], colour)?;
        }
        Ok(())
    }
}
fn div(a: i32, b: i32) -> Result<i32> {
    anyhow::ensure!(b != 0, "sprite division by zero");
    Ok(a.wrapping_div(b))
}
pub fn trig(angle: i32) -> [f32; 2] {
    let a = (angle & 65535) as f64 * 9.587379924285257E-5;
    [a.sin() as f32, a.cos() as f32]
}
