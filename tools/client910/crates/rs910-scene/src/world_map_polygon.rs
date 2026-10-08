//! Map-element polygon geometry shared by the world-map and minimap painters.
//!
//! * [`spans`] is the scanline fill with its edge table: 16.16 fixed-point
//!   edge walking, horizontal edges skipped, one span per active edge pair
//!   (start `x`, row `y`, length `x2 - x`).
//! * [`dashes`] is the styled-edge walk: the `phase % (dash + gap)` start,
//!   per-axis end clamping and the dash/gap walk used for styled polygon edges.
//! * [`edge_order`] is the endpoint canonicalisation that the world-map and
//!   minimap painters apply before every styled edge.

pub use rs910_model::line_dashes::*;

/// One horizontal line `(x, y, len)` of the scanline fill. `len` may be zero or
/// negative on the unmasked path, which emits such spans anyway.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub x: i32,
    pub y: i32,
    pub len: i32,
}

/// Edge-table state of the scanline fill.
struct Table {
    edges: Vec<i32>,
    count: usize,
    active_start: usize,
    active_end: usize,
    next_pair: usize,
    row: i32,
    left: i32,
    right: i32,
}

impl Table {
    /// Every non-horizontal edge from the previous point to the current one,
    /// stored top point first.
    fn build(points: &[i32]) -> Self {
        let mut edges = Vec::with_capacity(points.len() * 2 + 8);
        let n = points.len() - points.len() % 2;
        if n >= 2 {
            let mut prev = n - 2;
            let mut cur = 0;
            while cur < n {
                let (py, cy) = (points[prev + 1], points[cur + 1]);
                if py < cy {
                    edges.extend([points[prev], py, points[cur], cy]);
                } else if cy < py {
                    edges.extend([points[cur], cy, points[prev], py]);
                }
                prev = cur;
                cur += 2;
            }
        }
        let count = edges.len();
        // The pair after the last active edge is read from the same backing
        // array; keep one zeroed record so an odd active set reads defined
        // values instead of panicking.
        edges.extend([0; 8]);
        Self {
            edges,
            count,
            active_start: 0,
            active_end: 0,
            next_pair: 0,
            row: 0,
            left: 0,
            right: 0,
        }
    }

    /// 188-219: quicksort of 4-int records by top y.
    fn sort_by_top(&mut self, from: usize, to: usize) {
        if to <= from + 4 {
            return;
        }
        let e = &mut self.edges;
        let mut pivot = from;
        let saved = [e[from], e[from + 1], e[from + 2], e[from + 3]];
        let mut i = from + 4;
        while i < to {
            let y = e[i + 1];
            if y < saved[1] {
                e[pivot] = e[i];
                e[pivot + 1] = y;
                e[pivot + 2] = e[i + 2];
                e[pivot + 3] = e[i + 3];
                pivot += 4;
                e[i] = e[pivot];
                e[i + 1] = e[pivot + 1];
                e[i + 2] = e[pivot + 2];
                e[i + 3] = e[pivot + 3];
            }
            i += 4;
        }
        e[pivot..pivot + 4].copy_from_slice(&saved);
        self.sort_by_top(from, pivot);
        self.sort_by_top(pivot + 4, to);
    }

    /// 222-265: bubble sort of the active edges by current x.
    fn sort_active(&mut self, from: usize, mut to: usize) {
        let e = &mut self.edges;
        loop {
            if to < from + 8 {
                return;
            }
            let mut sorted = true;
            let mut i = from + 4;
            while i < to {
                if e[i - 4] > e[i] {
                    sorted = false;
                    e.swap(i - 4, i);
                    e.swap(i - 2, i + 2);
                    e.swap(i - 1, i + 3);
                }
                i += 4;
            }
            if sorted {
                return;
            }
            to -= 4;
        }
    }

    /// 123-154: sort, then start every edge beginning at or
    /// above the first visible row at that row.
    fn start(&mut self, clip_top: i32) {
        self.sort_by_top(0, self.count);
        let mut first = if self.count == 0 {
            clip_top
        } else {
            self.edges[1]
        };
        if first < clip_top {
            first = clip_top;
        }
        let mut i = 0;
        while i < self.count {
            let y0 = self.edges[i + 1];
            if first < y0 {
                break;
            }
            let (x0, x1, y1) = (self.edges[i], self.edges[i + 2], self.edges[i + 3]);
            let slope = ((x1 - x0) << 16).wrapping_div(y1 - y0);
            let x = (x0 << 16).wrapping_add(32768);
            self.edges[i] = (first - y0).wrapping_mul(slope).wrapping_add(x);
            self.edges[i + 2] = slope;
            i += 4;
        }
        self.active_start = 0;
        self.active_end = i;
        self.next_pair = i;
        self.row = first - 1;
    }

    /// 157-186: the next span, advancing rows as needed.
    fn next(&mut self, clip_bottom: i32) -> bool {
        let mut end = self.active_end;
        let mut pair = self.next_pair;
        let mut row = self.row;
        while pair >= end {
            row += 1;
            self.row = row;
            if row >= clip_bottom {
                return false;
            }
            let mut start = self.active_start;
            while end < self.count {
                let y0 = self.edges[end + 1];
                if row < y0 {
                    break;
                }
                let (x0, x1, y1) = (self.edges[end], self.edges[end + 2], self.edges[end + 3]);
                let slope = ((x1 - x0) << 16).wrapping_div(y1 - y0);
                self.edges[end] = (x0 << 16).wrapping_add(32768);
                self.edges[end + 2] = slope;
                end += 4;
            }
            let mut i = start;
            while i < end {
                if row >= self.edges[i + 3] {
                    for k in 0..4 {
                        self.edges[i + k] = self.edges[start + k];
                    }
                    start += 4;
                }
                i += 4;
            }
            if self.count == start {
                self.count = 0;
                return false;
            }
            self.sort_active(start, end);
            self.active_start = start;
            self.active_end = end;
            pair = start;
        }
        if pair + 6 >= self.edges.len() {
            return false;
        }
        self.left = self.edges[pair] >> 16;
        self.right = self.edges[pair + 4] >> 16;
        self.edges[pair] = self.edges[pair].wrapping_add(self.edges[pair + 2]);
        self.edges[pair + 4] = self.edges[pair + 4].wrapping_add(self.edges[pair + 6]);
        self.next_pair = pair + 8;
        true
    }
}

/// Scanline fill over the current clip rows `[top, bottom)`.
/// `mask` is `(starts, widths, clip_left)`: the graphic's per-row start and width
/// arrays (row 0 = clip top) that narrow each span; masked spans of zero or
/// negative length are skipped.
pub fn spans(
    points: &[i32],
    clip_top: i32,
    clip_bottom: i32,
    mask: Option<(&[i32], &[i32], i32)>,
) -> Vec<Span> {
    let mut out = Vec::new();
    if let Some((starts, _, _)) = mask {
        if clip_bottom - clip_top != starts.len() as i32 {
            return out;
        }
    }
    let mut table = Table::build(points);
    table.start(clip_top);
    // Guard against a malformed edge set looping without progress.
    let mut budget = (clip_bottom - clip_top).max(0) as usize * (table.count / 4 + 2) + 16;
    while table.next(clip_bottom) {
        budget = budget.saturating_sub(1);
        if budget == 0 {
            break;
        }
        let (mut x0, mut x1, y) = (table.left, table.right, table.row);
        if let Some((starts, widths, left)) = mask {
            let row = (y - clip_top) as usize;
            let (Some(&start), Some(&width)) = (starts.get(row), widths.get(row)) else {
                continue;
            };
            if x0 < start + left {
                x0 = start + left;
            }
            if x1 > start + width + left {
                x1 = start + width + left;
            }
            if x1 - x0 <= 0 {
                continue;
            }
        }
        out.push(Span {
            x: x0,
            y,
            len: x1 - x0,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_fill_rows_follow_fixed_point_edges() {
        // (10,10)-(20,10)-(20,20)-(10,20): left edge x=10, right x=20 for
        // rows 10..19; the bottom row is excluded by `row >= ymax`.
        let got = spans(&[10, 10, 20, 10, 20, 20, 10, 20], 0, 100, None);
        assert_eq!(got.len(), 10);
        assert!(got.iter().all(|s| s.x == 10 && s.len == 10));
        assert_eq!(got.first().unwrap().y, 10);
        assert_eq!(got.last().unwrap().y, 19);
    }

    #[test]
    fn triangle_fill_uses_half_pixel_biased_slopes() {
        // Apex (0,0), base (-8,8)..(8,8): x = (0<<16)+32768 +/- row<<16.
        let got = spans(&[0, 0, 8, 8, -8, 8], 0, 100, None);
        assert_eq!(got.len(), 8);
        for (row, span) in got.iter().enumerate() {
            let row = row as i32;
            assert_eq!(span.y, row);
            assert_eq!(span.x, ((32768 - (row << 16)) >> 16));
            assert_eq!(span.x + span.len, (32768 + (row << 16)) >> 16);
        }
    }

    #[test]
    fn clip_rows_and_mask_narrow_spans() {
        // Clip starts at row 12: edges start there, not at their top.
        let got = spans(&[10, 10, 20, 10, 20, 20, 10, 20], 12, 15, None);
        assert_eq!(got.iter().map(|s| s.y).collect::<Vec<_>>(), [12, 13, 14]);
        // Mask rows (start, width) relative to clip left 5.
        let starts = [7, 0, 20];
        let widths = [3, 100, 1];
        let got = spans(
            &[10, 10, 20, 10, 20, 20, 10, 20],
            12,
            15,
            Some((&starts, &widths, 5)),
        );
        assert_eq!(
            got,
            [
                Span {
                    x: 12,
                    y: 12,
                    len: 3
                },
                Span {
                    x: 10,
                    y: 13,
                    len: 10
                }
            ]
        );
        // A mask whose height mismatches the clip is invalid: nothing is filled.
        assert!(spans(&[0, 0, 4, 0, 4, 4], 0, 4, Some((&starts, &widths, 0))).is_empty());
    }
}
