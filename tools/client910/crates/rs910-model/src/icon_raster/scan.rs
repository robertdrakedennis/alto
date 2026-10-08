//! Triangle scan conversion: picks the top vertex, clips against the canvas,
//! walks the edges row by row and hands each row's span to the caller.
//!
//! Every fill mode shares this walker; a mode differs only in what it
//! interpolates ([`EdgeAttributes`]) and in how it fills a span. The float
//! arithmetic (evaluation order, the `+ 0.5` row rounding, the exact tie
//! rules below) is what produces the client's icon pixels, so none of it is
//! simplified.

/// A triangle corner on the canvas, in pixels.
#[derive(Clone, Copy)]
pub(super) struct Corner {
    pub x: f32,
    pub y: f32,
}

/// The three edges of a sorted triangle: `Long` runs from the top corner to
/// the bottom one; `Upper` and `Lower` are the two halves of the other side
/// (top to middle, middle to bottom).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Edge {
    Long,
    Upper,
    Lower,
}

/// Per-row values a fill mode interpolates while the walker steps down the
/// triangle.
pub(super) trait EdgeAttributes {
    /// What a span fill receives for one row.
    type Row;
    /// The corners have been sorted: `top`, `mid` and `bottom` index the
    /// corners in their original order. `top_x` is the top corner's x before
    /// any clipping.
    fn begin(&mut self, top: usize, mid: usize, bottom: usize, top_x: f32);
    /// The top corner lies above the canvas (`top_y < 0`): move the values
    /// on to row 0.
    fn clip_top(&mut self, top_y: f32);
    /// The middle corner lies above the canvas (`mid_y < 0`).
    fn clip_mid(&mut self, mid_y: f32);
    /// The values for the current row given which edge bounds each side.
    fn row(&self, left: Edge, right: Edge) -> Self::Row;
    /// Step to the next row; `lower` is set once the walk is on the
    /// middle-to-bottom edge.
    fn advance(&mut self, lower: bool);
}

/// How the walker breaks ties when it sorts the corners of a triangle. The
/// two fill families were written against different sorting rules, and the
/// differences show only where corners share a row after clipping.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum SortRule {
    /// Flat and Gouraud fills.
    Planar,
    /// Textured fills.
    Perspective,
}

/// Which side the long edge is on, decided from the slopes. Four variants of
/// the comparison exist; they differ in how equal slopes and a flat top edge
/// (`top_y == mid_y`) are treated.
#[derive(Clone, Copy)]
enum SideRule {
    /// Flat top edge decided by the lower slope; equal slopes put the long
    /// edge on the right.
    FlatTopLongRight,
    /// As above but equal slopes put the long edge on the left.
    FlatTopLongLeft,
    /// No flat-top case; equal slopes put the long edge on the left.
    PlainLongLeft,
    /// No flat-top case; equal slopes put the long edge on the right.
    PlainLongRight,
}

fn next(corner: usize) -> usize {
    (corner + 1) % 3
}

/// Calls `emit(row_start, left, right, row)` for every canvas row the
/// triangle covers, top to bottom. `left` and `right` are the span's pixel
/// bounds before horizontal clipping; `row_start` is the index of the row's
/// first pixel.
#[allow(
    clippy::neg_cmp_op_on_partial_ord,
    reason = "a NaN corner (a projected point at w = 0) must sort and side-pick exactly as it always has, which the negated forms do"
)]
pub(super) fn scan<A: EdgeAttributes>(
    corners: [Corner; 3],
    width: i32,
    height: i32,
    sort: SortRule,
    attributes: &mut A,
    mut emit: impl FnMut(i32, i32, i32, &A::Row),
) {
    let x = corners.map(|c| c.x);
    let y = corners.map(|c| c.y);
    // Slope of x against y along the edge between two corners; a
    // horizontal edge has slope 0.
    let slope = |a: usize, b: usize| {
        if y[a] == y[b] {
            0.0
        } else {
            (x[b] - x[a]) / (y[b] - y[a])
        }
    };
    let limit = height as f32;

    let top = if y[0] <= y[1] && y[0] <= y[2] {
        0
    } else if y[1] <= y[2] {
        1
    } else {
        2
    };
    if y[top] >= limit {
        return;
    }
    let (first, second) = (next(top), next(next(top)));
    // The two lower corners are clamped to the canvas bottom before they are
    // compared, so two corners below the canvas compare as equal.
    let clamp = |v: f32| if v > limit { limit } else { v };
    let (first_y, second_y) = (clamp(y[first]), clamp(y[second]));
    let mid_is_first = match (sort, top) {
        (SortRule::Perspective, 1 | 2) => second_y >= first_y,
        (SortRule::Perspective, _) => !(first_y >= second_y),
        (SortRule::Planar, _) => first_y < second_y,
    };
    let (mid, bottom, mid_y, bottom_y) = if mid_is_first {
        (first, second, first_y, second_y)
    } else {
        (second, first, second_y, first_y)
    };
    let side_rule = match (sort, top, mid_is_first) {
        (SortRule::Planar, 0, true) | (SortRule::Planar, 1, true) => SideRule::FlatTopLongRight,
        (SortRule::Planar, 0, false) => SideRule::FlatTopLongLeft,
        (SortRule::Planar, _, false) => SideRule::PlainLongLeft,
        (SortRule::Planar, _, true) => SideRule::PlainLongRight,
        (SortRule::Perspective, 0, true) => SideRule::FlatTopLongRight,
        (SortRule::Perspective, _, true) => SideRule::FlatTopLongLeft,
        (SortRule::Perspective, 0, false) => SideRule::FlatTopLongLeft,
        (SortRule::Perspective, _, false) => SideRule::FlatTopLongRight,
    };

    let upper_slope = slope(top, mid);
    let long_slope = slope(top, bottom);
    let lower_slope = slope(mid, bottom);
    attributes.begin(top, mid, bottom, x[top]);

    let mut long_x = x[top];
    let mut upper_x = x[top];
    let mut lower_x = x[mid];
    let mut top_y = y[top];
    let mut mid_y = mid_y;
    if top_y < 0.0 {
        long_x = x[top] - top_y * long_slope;
        upper_x = x[top] - top_y * upper_slope;
        attributes.clip_top(top_y);
        top_y = 0.0;
    }
    if mid_y < 0.0 {
        lower_x = x[mid] - mid_y * lower_slope;
        attributes.clip_mid(mid_y);
        mid_y = 0.0;
    }

    let long_is_left = match side_rule {
        SideRule::FlatTopLongRight => {
            (top_y != mid_y && long_slope < upper_slope)
                || (top_y == mid_y && long_slope > lower_slope)
        }
        SideRule::FlatTopLongLeft => {
            !((top_y != mid_y && upper_slope < long_slope)
                || (top_y == mid_y && lower_slope > long_slope))
        }
        SideRule::PlainLongLeft => !(upper_slope < long_slope),
        SideRule::PlainLongRight => long_slope < upper_slope,
    };

    // Rows are the pixel rows whose centre lies in [corner, next corner).
    let row_of = |v: f32| ((v + 0.5) as i32) as i64;
    let (top_row, mid_row, bottom_row) = (row_of(top_y), row_of(mid_y), row_of(bottom_y));
    let upper_rows = (mid_row - top_row).max(0);
    let lower_rows = (bottom_row - mid_row).max(0);
    let mut row_start = (top_row as i32).wrapping_mul(width);
    let (left_edge, right_edge, upper_left) = if long_is_left {
        (Edge::Long, Edge::Upper, false)
    } else {
        (Edge::Upper, Edge::Long, true)
    };
    for _ in 0..upper_rows {
        let (left, right) = if upper_left {
            (upper_x, long_x)
        } else {
            (long_x, upper_x)
        };
        emit(
            row_start,
            left as i32,
            right as i32,
            &attributes.row(left_edge, right_edge),
        );
        long_x += long_slope;
        upper_x += upper_slope;
        attributes.advance(false);
        row_start = row_start.wrapping_add(width);
    }
    let (left_edge, right_edge) = if long_is_left {
        (Edge::Long, Edge::Lower)
    } else {
        (Edge::Lower, Edge::Long)
    };
    for _ in 0..lower_rows {
        let (left, right) = if long_is_left {
            (long_x, lower_x)
        } else {
            (lower_x, long_x)
        };
        emit(
            row_start,
            left as i32,
            right as i32,
            &attributes.row(left_edge, right_edge),
        );
        long_x += long_slope;
        lower_x += lower_slope;
        attributes.advance(true);
        row_start = row_start.wrapping_add(width);
    }
}
