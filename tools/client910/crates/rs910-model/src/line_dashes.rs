//! Dash geometry of the toolkit's dashed line, and the styled-edge endpoint
//! order the world map and minimap apply before it. Split out of the scene
//! crate's `world_map_polygon` so the toolkit's `Painter::dashed_line` and
//! the map painters share it without the toolkit naming the scene layer;
//! `world_map_polygon` re-exports it.

/// Canonical styled-edge direction: left to right, or top to bottom for a
/// vertical edge.
pub fn edge_order(mut from: [i32; 2], mut to: [i32; 2]) -> ([i32; 2], [i32; 2]) {
    if to[0] < from[0] {
        std::mem::swap(&mut from, &mut to);
    } else if from[0] == to[0] && to[1] < from[1] {
        std::mem::swap(&mut from[1], &mut to[1]);
    }
    (from, to)
}

/// The visible dash segments of a dashed line as `[x0, y0, x1, y1]` floats.
/// `right_limit` bounds the degenerate zero-length edge, which the client
/// walks along +x until its vertex buffer fills (every such dash lies beyond
/// the clip anyway). `dash + gap == 0` is an arithmetic error there and
/// yields nothing here.
pub fn dashes(
    from: [i32; 2],
    to: [i32; 2],
    dash: i32,
    gap: i32,
    phase: i32,
    right_limit: i32,
) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    let cycle = dash.wrapping_add(gap);
    if cycle == 0 {
        return out;
    }
    let (x0, y0, x1, y1) = (from[0], from[1], to[0], to[1]);
    let mut ux = x1 as f32 - x0 as f32;
    let mut uy = y1 as f32 - y0 as f32;
    if ux == 0.0 && uy == 0.0 {
        ux = 1.0;
    } else {
        let inv = (1.0 / ((ux * ux + uy * uy) as f64).sqrt()) as f32;
        ux *= inv;
        uy *= inv;
    }
    // The 32-bit remainder keeps the dividend's sign.
    let rem = phase.wrapping_rem(cycle);
    let dash_x = dash as f32 * ux;
    let dash_y = dash as f32 * uy;
    let (mut off_x, mut off_y) = (0.0f32, 0.0f32);
    let (mut cur_x, mut cur_y) = (dash_x, dash_y);
    if rem > dash {
        off_x = (cycle - rem) as f32 * ux;
        off_y = (cycle - rem) as f32 * uy;
    } else {
        cur_x = (dash - rem) as f32 * ux;
        cur_y = (dash - rem) as f32 * uy;
    }
    let mut px = x0 as f32 + off_x;
    let mut py = y0 as f32 + off_y;
    let gap_x = gap as f32 * ux;
    let gap_y = gap as f32 * uy;
    let (fx1, fy1) = (x1 as f32, y1 as f32);
    let limit = right_limit as f32 + dash.abs() as f32 + gap.abs() as f32;
    loop {
        if x1 > x0 {
            if px > fx1 {
                break;
            }
            if cur_x + px > fx1 {
                cur_x = fx1 - px;
            }
        } else {
            if px < fx1 {
                break;
            }
            if cur_x + px < fx1 {
                cur_x = fx1 - px;
            }
        }
        if y1 > y0 {
            if py > fy1 {
                break;
            }
            if cur_y + py > fy1 {
                cur_y = fy1 - py;
            }
        } else {
            if py < fy1 {
                break;
            }
            if cur_y + py < fy1 {
                cur_y = fy1 - py;
            }
        }
        if px > limit || !px.is_finite() || !py.is_finite() {
            break;
        }
        out.push([px, py, px + cur_x, py + cur_y]);
        px += cur_x + gap_x;
        py += cur_y + gap_y;
        cur_x = dash_x;
        cur_y = dash_y;
        if out.len() > 1 << 16 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn styled_edges_follow_the_dash_phase_and_clamp() {
        // Horizontal 0..10, dash 3 gap 2 phase 0: [0,3] [5,8] [10,10].
        let got = dashes([0, 0], [10, 0], 3, 2, 0, 1000);
        assert_eq!(
            got,
            [[0., 0., 3., 0.], [5., 0., 8., 0.], [10., 0., 10., 0.]]
        );
        // phase 1 <= dash: the first dash is shortened by the phase.
        let got = dashes([0, 0], [10, 0], 3, 2, 1, 1000);
        assert_eq!(got[0], [0., 0., 2., 0.]);
        assert_eq!(got[1], [4., 0., 7., 0.]);
        // phase 4 > dash: start after the rest of the gap.
        let got = dashes([0, 0], [10, 0], 3, 2, 4, 1000);
        assert_eq!(got[0], [1., 0., 4., 0.]);
        // `%` keeps the dividend's sign: phase -1 lengthens the first dash.
        let got = dashes([0, 0], [10, 0], 3, 2, -1, 1000);
        assert_eq!(got[0], [0., 0., 4., 0.]);
        // Vertical: clamp against y.
        let got = dashes([5, 0], [5, 4], 3, 2, 0, 1000);
        assert_eq!(got, [[5., 0., 5., 3.]]);
        // dash + gap == 0 has no defined result.
        assert!(dashes([0, 0], [10, 0], 3, -3, 0, 1000).is_empty());
        // Degenerate edge walks right until the clip limit.
        let got = dashes([0, 0], [0, 0], 3, 2, 0, 20);
        assert!(!got.is_empty() && got.iter().all(|d| d[1] == 0. && d[0] <= 25.));
    }

    #[test]
    fn styled_edges_are_canonicalised_before_drawing() {
        assert_eq!(edge_order([10, 3], [2, 7]), ([2, 7], [10, 3]));
        assert_eq!(edge_order([4, 9], [4, 1]), ([4, 1], [4, 9]));
        assert_eq!(edge_order([1, 1], [4, 1]), ([1, 1], [4, 1]));
    }
}
