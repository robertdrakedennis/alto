//! What a fill interpolates across a triangle.
//!
//! Flat and Gouraud fills interpolate linear attributes (depth, palette
//! index, colour channels) from a plane fitted through the three corners.
//! Textured fills interpolate perspective-correct attributes edge by edge
//! (each edge carries its own values and per-row steps).

use super::scan::{Corner, Edge, EdgeAttributes};

/// `N` attributes that vary linearly over the triangle. The per-pixel
/// gradient (`across`) is constant; the row's starting value is stepped down
/// by `down` each row.
pub(super) struct Planar<const N: usize> {
    corner_values: [[f32; 3]; N],
    pub across: [f32; N],
    down: [f32; N],
    start: [f32; N],
}

impl<const N: usize> Planar<N> {
    /// Fits the plane through `values` (one array of three per attribute, in
    /// corner order). `None` when the triangle has no area.
    pub fn new(corners: [Corner; 3], values: [[f32; 3]; N]) -> Option<Self> {
        let edge_x1 = corners[1].x - corners[0].x;
        let edge_y1 = corners[1].y - corners[0].y;
        let edge_x2 = corners[2].x - corners[0].x;
        let edge_y2 = corners[2].y - corners[0].y;
        let area = edge_x1 * edge_y2 - edge_y1 * edge_x2;
        if area == 0.0 {
            return None;
        }
        let mut across = [0.0; N];
        let mut down = [0.0; N];
        for (k, v) in values.iter().enumerate() {
            let d1 = v[1] - v[0];
            let d2 = v[2] - v[0];
            across[k] = (edge_y2 * d1 - edge_y1 * d2) / area;
            down[k] = (edge_x1 * d2 - edge_x2 * d1) / area;
        }
        Some(Self {
            corner_values: values,
            across,
            down,
            start: [0.0; N],
        })
    }
}

impl<const N: usize> EdgeAttributes for Planar<N> {
    type Row = [f32; N];

    fn begin(&mut self, top: usize, _mid: usize, _bottom: usize, top_x: f32) {
        for ((start, values), across) in self
            .start
            .iter_mut()
            .zip(&self.corner_values)
            .zip(&self.across)
        {
            *start = (values[top] - top_x * across) + across;
        }
    }

    fn clip_top(&mut self, top_y: f32) {
        for (start, down) in self.start.iter_mut().zip(&self.down) {
            *start -= top_y * down;
        }
    }

    fn clip_mid(&mut self, _mid_y: f32) {}

    fn row(&self, _left: Edge, _right: Edge) -> [f32; N] {
        self.start
    }

    fn advance(&mut self, _lower: bool) {
        for (start, down) in self.start.iter_mut().zip(&self.down) {
            *start += down;
        }
    }
}

/// The attributes a textured fill interpolates, in this order: reciprocal
/// depth, reciprocal w, u / w, v / w, alpha, red, green, blue.
pub(super) const TEXTURED_ATTRIBUTES: usize = 8;
/// Index of the alpha channel among [`TEXTURED_ATTRIBUTES`]; red, green and
/// blue follow it.
const ALPHA: usize = 4;

pub(super) type Attributes = [f32; TEXTURED_ATTRIBUTES];

/// `target += step`, attribute by attribute.
pub(super) fn add(target: &mut Attributes, step: &Attributes) {
    for (t, s) in target.iter_mut().zip(step) {
        *t += s;
    }
}

/// `target -= distance * step`, attribute by attribute.
pub(super) fn subtract_scaled(target: &mut Attributes, distance: f32, step: &Attributes) {
    for (t, s) in target.iter_mut().zip(step) {
        *t -= distance * s;
    }
}

/// `(end - start) * factor`, attribute by attribute.
pub(super) fn scaled_difference(end: &Attributes, start: &Attributes, factor: f32) -> Attributes {
    std::array::from_fn(|k| (end[k] - start[k]) * factor)
}

/// Perspective attributes stepped edge by edge.
pub(super) struct PerEdge {
    corner_values: [Attributes; 3],
    /// Attribute slope (change per row) of the edge between two corners,
    /// indexed by [`PerEdge::pair`].
    pair_slopes: [Attributes; 3],
    top: usize,
    mid: usize,
    bottom: usize,
    long: Attributes,
    upper: Attributes,
    lower: Attributes,
}

impl PerEdge {
    pub fn new(corner_y: [f32; 3], corner_values: [Attributes; 3]) -> Self {
        let pair_slope = |a: usize, b: usize| {
            let mut slopes = [0.0; TEXTURED_ATTRIBUTES];
            if corner_y[a] != corner_y[b] {
                let dy = corner_y[b] - corner_y[a];
                for (k, s) in slopes.iter_mut().enumerate() {
                    *s = (corner_values[b][k] - corner_values[a][k]) / dy;
                }
            }
            slopes
        };
        Self {
            corner_values,
            pair_slopes: [pair_slope(0, 1), pair_slope(1, 2), pair_slope(0, 2)],
            top: 0,
            mid: 1,
            bottom: 2,
            long: [0.0; TEXTURED_ATTRIBUTES],
            upper: [0.0; TEXTURED_ATTRIBUTES],
            lower: [0.0; TEXTURED_ATTRIBUTES],
        }
    }

    /// Index into `pair_slopes` for the edge between two corners.
    fn pair(a: usize, b: usize) -> usize {
        match (a.min(b), a.max(b)) {
            (0, 1) => 0,
            (1, 2) => 1,
            _ => 2,
        }
    }

    fn slopes(&self, a: usize, b: usize) -> &Attributes {
        &self.pair_slopes[Self::pair(a, b)]
    }
}

impl EdgeAttributes for PerEdge {
    type Row = (Attributes, Attributes);

    fn begin(&mut self, top: usize, mid: usize, bottom: usize, _top_x: f32) {
        self.top = top;
        self.mid = mid;
        self.bottom = bottom;
        self.long = self.corner_values[top];
        self.upper = self.corner_values[top];
        self.lower = self.corner_values[mid];
    }

    /// Quirk: when the top corner is above the canvas, the colour channels
    /// (not just alpha) step by the alpha slope of their edge, so a clipped
    /// textured face is tinted from the wrong gradient.
    fn clip_top(&mut self, top_y: f32) {
        let upper = *self.slopes(self.top, self.mid);
        let long = *self.slopes(self.top, self.bottom);
        // Every channel from alpha on takes the alpha slope.
        let alpha_slopes = |slopes: Attributes| std::array::from_fn(|k| slopes[k.min(ALPHA)]);
        subtract_scaled(&mut self.upper, top_y, &alpha_slopes(upper));
        subtract_scaled(&mut self.long, top_y, &alpha_slopes(long));
    }

    fn clip_mid(&mut self, mid_y: f32) {
        let lower = *self.slopes(self.mid, self.bottom);
        subtract_scaled(&mut self.lower, mid_y, &lower);
    }

    fn row(&self, left: Edge, right: Edge) -> Self::Row {
        let pick = |edge| match edge {
            Edge::Long => self.long,
            Edge::Upper => self.upper,
            Edge::Lower => self.lower,
        };
        (pick(left), pick(right))
    }

    fn advance(&mut self, lower: bool) {
        let long = *self.slopes(self.top, self.bottom);
        let short = if lower {
            *self.slopes(self.mid, self.bottom)
        } else {
            *self.slopes(self.top, self.mid)
        };
        add(&mut self.long, &long);
        add(
            if lower {
                &mut self.lower
            } else {
                &mut self.upper
            },
            &short,
        );
    }
}
