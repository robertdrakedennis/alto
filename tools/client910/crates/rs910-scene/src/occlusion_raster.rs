//! The occlusion depth raster: a coarse depth buffer (one cell stands for a
//! 3x3 block of screen pixels) that the occluder quads are drawn into and
//! that visibility queries are tested against.
//!
//! All arithmetic is 32-bit and wraps: edge positions are 12-bit fixed point,
//! depth gradients 8-bit fixed point, and the results of the wrapped values
//! decide what the scene draws, so they are reproduced bit for bit rather
//! than "corrected". Each triangle grows by one cell on every side (the
//! conservative expansion) so a query never misses an edge cell.

/// What the raster does with a triangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterMode {
    /// No pass is running. Triangles are tested like [`RasterMode::Test`],
    /// but the corner pre-check is skipped.
    Idle,
    /// The occluders are being drawn: each covered cell keeps the nearest
    /// depth and the covered-cell count grows.
    Write,
    /// A query: the triangle is occluded when every cell it touches is
    /// behind what was drawn.
    Test,
}

impl RasterMode {
    /// The value the draw trace records for the mode.
    pub fn code(self) -> i32 {
        match self {
            Self::Idle => 0,
            Self::Write => 1,
            Self::Test => 2,
        }
    }
}

/// A triangle in raster cells: per corner the row, the column and the depth.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RasterTriangle {
    pub rows: [i32; 3],
    pub cols: [i32; 3],
    pub depths: [i32; 3],
}

impl RasterTriangle {
    /// The nine values in the order the draw trace records them: three rows,
    /// three columns, three depths.
    pub fn words(&self) -> [i32; 9] {
        let [r0, r1, r2] = self.rows;
        let [c0, c1, c2] = self.cols;
        let [d0, d1, d2] = self.depths;
        [r0, r1, r2, c0, c1, c2, d0, d1, d2]
    }

    pub fn from_words(w: [i32; 9]) -> Self {
        Self {
            rows: [w[0], w[1], w[2]],
            cols: [w[3], w[4], w[5]],
            depths: [w[6], w[7], w[8]],
        }
    }
}

/// Corner coordinates beyond this many cells from the origin are rejected.
const GUARD_BAND: i32 = 2003;
/// A query cell counts as behind the drawn depth only by more than this
/// (depth is in 1/256 units).
const QUERY_DEPTH_MARGIN: i32 = 38656;
/// Fixed-point shifts of the edge positions and of the depth gradients.
const EDGE_SHIFT: u32 = 12;
const GRADIENT_SHIFT: u32 = 8;

#[derive(Clone, Debug)]
pub struct DepthRaster {
    pub width: i32,
    pub height: i32,
    pub depth: Vec<i32>,
    pub mode: RasterMode,
    /// Cells written this pass, counted four at a time per span.
    pub coverage: i32,
}

/// The three edges of a triangle by where they run: the long edge from the
/// top corner to the bottom one and the two short ones that meet at the middle
/// corner.
struct Edges {
    /// Column slope per row, 12-bit fixed point, of the long edge (top to
    /// bottom), the first short edge (top to middle) and the second (middle
    /// to bottom).
    long: i32,
    upper: i32,
    lower: i32,
}

impl DepthRaster {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            depth: vec![0; (width * height) as usize],
            mode: RasterMode::Idle,
            coverage: 0,
        }
    }

    /// Draw (mode `Write`) or test (mode `Test`) one triangle. Returns
    /// `false` when the triangle is rejected: a corner outside the guard
    /// band, a degenerate triangle, or (testing) a cell in front of what was
    /// drawn. A triangle wholly below the raster counts as covered.
    pub fn triangle(&mut self, tri: RasterTriangle) -> bool {
        let RasterTriangle {
            mut rows,
            cols,
            depths,
        } = tri;
        if rows
            .iter()
            .chain(&cols)
            .any(|&v| !(-GUARD_BAND..=GUARD_BAND).contains(&v))
        {
            return false;
        }
        if self.mode == RasterMode::Test && self.corner_hidden(&rows, &cols, &depths) {
            return false;
        }
        // Edge and depth-plane vectors from corner 0, before the expansion.
        let (col_a, row_a) = (cols[1].wrapping_sub(cols[0]), rows[1].wrapping_sub(rows[0]));
        let (col_b, row_b) = (cols[2].wrapping_sub(cols[0]), rows[2].wrapping_sub(rows[0]));
        let (depth_a, depth_b) = (
            depths[1].wrapping_sub(depths[0]),
            depths[2].wrapping_sub(depths[0]),
        );
        expand_rows(&mut rows);
        let edge_slope = |from: usize, to: usize| -> i32 {
            if rows[from] == rows[to] {
                0
            } else {
                (cols[to].wrapping_sub(cols[from]) << EDGE_SHIFT)
                    .wrapping_div(rows[to].wrapping_sub(rows[from]))
            }
        };
        let slopes = [edge_slope(0, 1), edge_slope(1, 2), edge_slope(2, 0)];
        let area = col_a
            .wrapping_mul(row_b)
            .wrapping_sub(row_a.wrapping_mul(col_b));
        if area == 0 {
            return false;
        }
        let across = (row_b
            .wrapping_mul(depth_a)
            .wrapping_sub(row_a.wrapping_mul(depth_b))
            << GRADIENT_SHIFT)
            .wrapping_div(area);
        let down = (col_a
            .wrapping_mul(depth_b)
            .wrapping_sub(col_b.wrapping_mul(depth_a))
            << GRADIENT_SHIFT)
            .wrapping_div(area);

        let order = sort_corners(&rows);
        let slope_between = |a: usize, b: usize| -> i32 {
            // The slope table is indexed by the edge 0-1, 1-2, 2-0.
            match (a.min(b), a.max(b)) {
                (0, 1) => slopes[0],
                (1, 2) => slopes[1],
                _ => slopes[2],
            }
        };
        let [top, middle, bottom] = order;
        let edges = Edges {
            long: slope_between(top, bottom),
            upper: slope_between(top, middle),
            lower: slope_between(middle, bottom),
        };
        if rows[top] >= self.height {
            return true;
        }
        for row in &mut rows {
            *row = (*row).min(self.height);
        }
        // Depth at column 0 of the top row, one column ahead.
        let mut depth_at_origin = (depths[top] << GRADIENT_SHIFT)
            .wrapping_sub(cols[top].wrapping_mul(across))
            .wrapping_add(across);
        // Both edges leaving the top corner start at its column; a top corner
        // above the raster advances them (and the depth) to row 0.
        let mut long_edge = cols[top] << EDGE_SHIFT;
        let mut upper_edge = long_edge;
        let mut top_row = rows[top];
        if top_row < 0 {
            long_edge = long_edge.wrapping_sub(top_row.wrapping_mul(edges.long));
            upper_edge = upper_edge.wrapping_sub(top_row.wrapping_mul(edges.upper));
            depth_at_origin = depth_at_origin.wrapping_sub(top_row.wrapping_mul(down));
            top_row = 0;
        }
        let mut middle_edge = cols[middle] << EDGE_SHIFT;
        let mut middle_row = rows[middle];
        if middle_row < 0 {
            middle_edge = middle_edge.wrapping_sub(middle_row.wrapping_mul(edges.lower));
            middle_row = 0;
        }
        let bottom_row = rows[bottom];
        let long_on_left = long_edge_on_left(&order, &edges, top_row, middle_row);

        let mut cursor = Cursor {
            row_offset: self.width.wrapping_mul(top_row),
            depth: depth_at_origin,
            across,
            down,
        };
        // Upper part: from the top corner to the middle corner.
        for _ in 0..middle_row.wrapping_sub(top_row).max(0) {
            let (left, right) = if long_on_left {
                (long_edge, upper_edge)
            } else {
                (upper_edge, long_edge)
            };
            if !self.span_of_row(&cursor, left, right) {
                return false;
            }
            long_edge = long_edge.wrapping_add(edges.long);
            upper_edge = upper_edge.wrapping_add(edges.upper);
            cursor.next_row(self.width);
        }
        // Lower part: the second short edge takes over from the first.
        for _ in 0..bottom_row.wrapping_sub(middle_row).max(0) {
            let (left, right) = if long_on_left {
                (long_edge, middle_edge)
            } else {
                (middle_edge, long_edge)
            };
            if !self.span_of_row(&cursor, left, right) {
                return false;
            }
            long_edge = long_edge.wrapping_add(edges.long);
            middle_edge = middle_edge.wrapping_add(edges.lower);
            cursor.next_row(self.width);
        }
        true
    }

    /// Testing only: whether a corner cell of the triangle already holds a
    /// nearer depth than the corner, which occludes the whole triangle.
    fn corner_hidden(&self, rows: &[i32; 3], cols: &[i32; 3], depths: &[i32; 3]) -> bool {
        let len = self.depth.len() as i32;
        (0..3).any(|corner| {
            let index = self
                .width
                .wrapping_mul(rows[corner])
                .wrapping_add(cols[corner]);
            (0..len).contains(&index)
                && (depths[corner] << GRADIENT_SHIFT).wrapping_sub(QUERY_DEPTH_MARGIN)
                    < self.depth[index as usize]
        })
    }

    /// One scan row between two edge positions (12-bit fixed point), grown by
    /// one cell on each side.
    fn span_of_row(&mut self, cursor: &Cursor, left: i32, right: i32) -> bool {
        self.span(
            cursor.row_offset,
            (left >> EDGE_SHIFT) - 1,
            (right >> EDGE_SHIFT) + 1,
            cursor.depth,
            cursor.across,
        )
    }

    /// Cells `start..end` of one row: raise the depth (`Write`) or check
    /// against it. The depth of cell `x` is `depth_at_origin + x * step`.
    fn span(
        &mut self,
        row_offset: i32,
        start: i32,
        end: i32,
        depth_at_origin: i32,
        step: i32,
    ) -> bool {
        let end = end.min(self.width);
        let start = start.max(0);
        if start >= end {
            return true;
        }
        let mut depth = start.wrapping_mul(step).wrapping_add(depth_at_origin);
        let writing = self.mode == RasterMode::Write;
        if writing {
            self.coverage = self.coverage.wrapping_add((end - start) >> 2);
        } else {
            depth = depth.wrapping_sub(QUERY_DEPTH_MARGIN);
        }
        for x in start..end {
            let cell = row_offset.wrapping_add(x) as usize;
            if writing {
                if depth < self.depth[cell] {
                    self.depth[cell] = depth;
                }
            } else if depth < self.depth[cell] {
                return false;
            }
            depth = depth.wrapping_add(step);
        }
        true
    }
}

/// The scan position: the offset of the row's first cell and the depth of
/// its column 0, with the per-column and per-row depth gradients.
struct Cursor {
    row_offset: i32,
    depth: i32,
    across: i32,
    down: i32,
}

impl Cursor {
    fn next_row(&mut self, width: i32) {
        self.depth = self.depth.wrapping_add(self.down);
        self.row_offset = self.row_offset.wrapping_add(width);
    }
}

/// Grow the triangle by one row at the top and one at the bottom: the
/// topmost corner moves up, and the lower of the other two moves down (the
/// later corner when they tie).
fn expand_rows(rows: &mut [i32; 3]) {
    let [r0, r1, r2] = *rows;
    if r0 < r1 && r0 < r2 {
        rows[0] = r0.wrapping_sub(1);
        if r1 > r2 {
            rows[1] = r1.wrapping_add(1);
        } else {
            rows[2] = r2.wrapping_add(1);
        }
    } else if r1 < r2 {
        rows[1] = r1.wrapping_sub(1);
        if r0 > r2 {
            rows[0] = r0.wrapping_add(1);
        } else {
            rows[2] = r2.wrapping_add(1);
        }
    } else {
        rows[2] = r2.wrapping_sub(1);
        if r0 > r1 {
            rows[0] = r0.wrapping_add(1);
        } else {
            rows[1] = r1.wrapping_add(1);
        }
    }
}

/// Corner indices ordered top, middle, bottom by row. Ties keep the order
/// the six cases below fall out in; the side rule depends on it.
fn sort_corners(rows: &[i32; 3]) -> [usize; 3] {
    let [r0, r1, r2] = *rows;
    if r0 <= r1 && r0 <= r2 {
        if r1 < r2 {
            [0, 1, 2]
        } else {
            [0, 2, 1]
        }
    } else if r1 > r2 {
        if r0 < r1 {
            [2, 0, 1]
        } else {
            [2, 1, 0]
        }
    } else if r2 >= r0 {
        [1, 0, 2]
    } else {
        [1, 2, 0]
    }
}

/// Whether the long edge is the left boundary of the triangle. The two edges
/// leaving the top corner start at the same column, so the one with the
/// smaller slope is on the left; which edge that is at a tie (equal slopes,
/// or the top and middle corners in the same row) differs by corner order and
/// is kept exactly, because it decides which cells a triangle touches.
fn long_edge_on_left(order: &[usize; 3], edges: &Edges, top_row: i32, middle_row: i32) -> bool {
    let Edges {
        long, upper, lower, ..
    } = *edges;
    let same_row = top_row == middle_row;
    match (order[0], order[1], order[2]) {
        (0, 1, 2) | (1, 2, 0) => (!same_row && long < upper) || (same_row && long > lower),
        (0, 2, 1) => (same_row || upper >= long) && (!same_row || lower <= long),
        (2, 0, 1) => long < upper,
        _ => upper >= long,
    }
}
