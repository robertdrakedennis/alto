//! Tests of the NXT terrain builder (renderer plan M10), CPU only and
//! without the pack; the pack and GPU tests are `crate::frame::tests`'.
use super::*;

/// Is `p` strictly inside the triangle `t` (x east, z north)?
fn inside(t: &[Point; 3], p: Point) -> bool {
    let side = |a: Point, b: Point| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let s = [side(t[0], t[1]), side(t[1], t[2]), side(t[2], t[0])];
    s.iter().all(|&v| v > 1e-6) || s.iter().all(|&v| v < -1e-6)
}

fn signed_area(t: &[Point; 3]) -> f32 {
    0.5 * ((t[1][0] - t[0][0]) * (t[2][1] - t[0][1]) - (t[2][0] - t[0][0]) * (t[1][1] - t[0][1]))
}

/// Every overlay shape at every rotation covers its tile exactly once
/// (sampled), with counter-clockwise triangles; shapes 1-11 have an
/// overlay part and an underlay part; there is no shape 12.
#[test]
fn shapes_partition_the_tile() {
    for shape in 0..=11u8 {
        let tris = shape_triangles(shape).unwrap();
        let parts: std::collections::BTreeSet<_> =
            tris.iter().map(|t| format!("{:?}", t.0)).collect();
        if shape == 0 {
            assert_eq!(parts.len(), 1);
        } else {
            assert!(parts.contains("A") && parts.contains("B"), "shape {shape}");
        }
        for rotation in 0..4 {
            let rotated: Vec<[Point; 3]> = tris
                .iter()
                .map(|t| t.1.map(|p| rotate(p, rotation)))
                .collect();
            let area: f32 = rotated.iter().map(signed_area).sum();
            assert!(
                (area - 1.0).abs() < 1e-5,
                "shape {shape} r{rotation}: area {area}"
            );
            assert!(
                rotated.iter().all(|t| signed_area(t) > 0.0),
                "shape {shape}: winding"
            );
            for i in 0..23 {
                for j in 0..23 {
                    let p = [(i as f32 + 0.313) / 23.0, (j as f32 + 0.571) / 23.0];
                    let n = rotated.iter().filter(|t| inside(t, p)).count();
                    assert_eq!(n, 1, "shape {shape} r{rotation} at {p:?}");
                }
            }
        }
    }
    assert!(shape_triangles(12).is_none());
}

#[test]
fn rotation_cycles_the_corners() {
    let corners = [SW, NW, NE, SE];
    for (j, &c) in corners.iter().enumerate() {
        for r in 0..4u8 {
            assert_eq!(rotate(c, r), corners[(j + usize::from(r)) % 4]);
        }
    }
    assert_eq!(rotate(SM, 1), WM);
}

/// The `74 / 127` lightness is the classic floor lightness at the unshadowed
/// shade (`(l * 74) >> 7`) within one step, and the colour is the classic
/// table's at that lightness.
#[test]
fn terrain_colour_is_the_classic_floor_lightness() {
    let table = &rs910_core::colour::hsl_tables().rgb;
    for hsl in (0..=0xFFFF_u32).step_by(7) {
        let l = hsl & 0x7F;
        let ours = l * 74 / 127;
        let classic = (l * 74) >> 7;
        assert!(ours.abs_diff(classic) <= 1, "{hsl:#x}");
        let c = terrain_colour(hsl as u16, None);
        let want = table[((hsl & 0xFF80) | ours) as usize];
        assert_eq!(c, [(want >> 16) as u8, (want >> 8) as u8, want as u8, 255]);
    }
}

/// A level mesh's indices over a selection are its selected tiles'
/// ranges, in the selection's order.
#[test]
fn selection_picks_the_tiles_ranges() {
    let mesh = LevelMesh {
        level: 0,
        tiles: [2, 2],
        vertices: vec![TerrainVertex::default(); 12],
        indices: (0..12).collect(),
        tile_ranges: vec![(0, 3), (3, 3), (6, 3), (9, 3)],
        grid: Vec::new(),
    };
    let selection = FloorSelection {
        whole: false,
        origin: [0, 0],
        distance: 1,
        mask: vec![
            vec![true, false, false],
            vec![true, true, false],
            vec![false; 3],
        ],
    };
    // Tiles (0,0) and (1,0), (1,1): ids z * 2 + x = 0, 1, 3.
    assert_eq!(mesh.select(&selection), vec![0, 1, 2, 3, 4, 5, 9, 10, 11]);
    assert!(mesh.has_tile(1, 1) && mesh.has_tile(0, 1));
}
