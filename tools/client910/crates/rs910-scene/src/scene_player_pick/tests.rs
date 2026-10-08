use super::*;
fn p(id: i32, d: i32) -> PickablePlayer {
    let matrix = [
        1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ];
    PickablePlayer {
        id: PlayerPickId {
            pid: id,
            generation: 1,
        },
        bounds: ModelBounds {
            min: [-1, -1, -1],
            max: [1, 1, 1],
        },
        screen_bounds: None,
        projected_depth: d,
        matrix,
        active: true,
    }
}
/// Each pickable is inserted before the first entry whose depth is not greater, so depth
/// descending with equal depths newer first. The input order is neither
/// the depth order nor its reverse, and holds a tie, so neither a plain
/// reverse nor a stable depth-only sort passes.
#[test]
fn depth_order() {
    let players = [p(1, 20), p(2, 10), p(3, 30), p(4, 20)];
    let q = pick_players(&players, [0., 0., 100., 100.], [50, 50], [0, 0]);
    assert_eq!(
        q.iter().map(|x| x.pid).collect::<Vec<_>>(),
        vec![3, 4, 1, 2]
    );
}
#[test]
fn equal_depth_newer_first() {
    let a = p(1, 4);
    let b = p(2, 4);
    let q = pick_players(&[a, b], [0., 0., 100., 100.], [50, 50], [0, 0]);
    assert_eq!(q.iter().map(|x| x.pid).collect::<Vec<_>>(), vec![2, 1]);
}
/// Screen = `v + v.half * clip / w` with the scale below maps model
/// units one-to-one onto pixels around (400, 300).
fn pixel_mvp() -> [f32; 16] {
    let mut m = [0.; 16];
    m[0] = 1. / 400.;
    m[5] = 1. / 300.;
    m[10] = 1.;
    m[15] = 1.;
    m
}
const V: [f32; 4] = [400., 300., 400., 300.];

/// Two separated triangles inside one bounding box: (0..40)^2 and
/// (60..100)^2 in the XY plane.
fn two_triangles() -> crate::gpumodel::GpuModel {
    let mut raw = crate::modelunlit::ModelUnlit {
        version: 12,
        ..Default::default()
    };
    for (x, y) in [(0, 0), (40, 0), (0, 40), (60, 60), (100, 60), (60, 100)] {
        raw.vertex_x.push(x);
        raw.vertex_y.push(y);
        raw.vertex_z.push(0);
        raw.vertex_count += 1;
    }
    raw.used_vertex_count = raw.vertex_count;
    for [a, b, c] in [[0i16, 1, 2], [3, 4, 5]] {
        raw.face_vertex1.push(a);
        raw.face_vertex2.push(b);
        raw.face_vertex3.push(c);
        raw.face_colour.push(100);
        raw.face_count += 1;
    }
    crate::gpumodel::GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: &crate::texture::MaterialStore::default(),
            billboards: &crate::billboard::BillboardStore::default(),
            emitters: &crate::particle::EmitterStore::default(),
        },
        &raw,
        crate::gpumodel::BuildParams {
            flags: 0,
            ambient: 64,
            contrast: 768,
            detail: 0,
        },
    )
    .unwrap()
}

#[test]
fn model_pick_boxes_then_tests_projected_face_bounds() {
    let mut model = two_triangles();
    let m = pixel_mvp();
    // Inside the first face's screen box.
    assert!(model_pick(&mut model, &m, V, [410, 310], false, 0));
    // Inside the second face's box.
    assert!(model_pick(&mut model, &m, V, [480, 380], false, 0));
    // Between the faces: the model box hits, no face box does.
    assert!(model_pick(&mut model, &m, V, [450, 350], true, 0));
    assert!(!model_pick(&mut model, &m, V, [450, 350], false, 0));
    // The face test is a bounding-box test, not point-in-triangle: the
    // far corner of the first face's box still hits.
    assert!(model_pick(&mut model, &m, V, [438, 338], false, 0));
    // Outside the model box (strict float comparison at the edge).
    assert!(!model_pick(&mut model, &m, V, [400, 350], false, 0));
    assert!(!model_pick(&mut model, &m, V, [520, 350], false, 0));
}

#[test]
fn model_pick_drops_faces_behind_the_near_plane() {
    let mut model = two_triangles();
    let mut m = pixel_mvp();
    // tz = 0.2 - 0.01 * (x + y): vertices with x + y > 120 fall behind
    // -w. Only the (100, 100) box corner and the second face's
    // (100, 60)/(60, 100) vertices are dropped.
    m[2] = -0.01;
    m[6] = -0.01;
    m[14] = 0.2;
    assert!(model_pick(&mut model, &m, V, [410, 310], false, 0));
    assert!(!model_pick(&mut model, &m, V, [480, 380], false, 0));
}

#[test]
fn clickbox_pick_uses_projected_cuboid_bounds() {
    let m = pixel_mvp();
    let c = [-10., 0., -5., 30., 20., 5.];
    assert!(cuboid_pick(c, &m, V, [400, 310]));
    assert!(cuboid_pick(c, &m, V, [425, 301]));
    assert!(!cuboid_pick(c, &m, V, [390, 310]));
    assert!(!cuboid_pick(c, &m, V, [431, 310]));
    // Strict edges.
    assert!(!cuboid_pick(c, &m, V, [430, 310]));
    assert!(!cuboid_pick(c, &m, V, [410, 300]));
}

#[test]
fn clickbox_capsule_projects_the_centre_column_and_half_diagonal() {
    let m = pixel_mvp();
    // `projection` is a perspective projection: no constant w term, so the
    // offset column adds only the clip-space w of the endpoint.
    let mut projection = m;
    projection[15] = 0.;
    let b = cuboid_screen_bounds([-10., 0., -5., 30., 20., 5.], m, projection, V);
    assert!(b.enabled);
    // Centre column x = (int)(-10 + 30) >> 1 = 10, from minY to maxY.
    assert_eq!(b.a, [410, 300]);
    assert_eq!(b.b, [410, 320]);
    // Half diagonal sqrt(40^2 + 10^2) / 2 = 20.6; through the unit
    // projection it adds 20.6 * 400 / 400 px to the endpoint.
    assert_eq!(b.radius, 20);
    assert!(capsule_hit(b, [410, 310], [0, 0]));
    assert!(!capsule_hit(b, [440, 310], [0, 0]));
    // Both ends behind the near plane disable the capsule.
    let mut behind = m;
    behind[14] = -2.;
    assert!(!cuboid_screen_bounds([-10., 0., -5., 30., 20., 5.], behind, projection, V).enabled);
}

#[test]
fn draw_radius_uses_integer_endpoint_after_projection() {
    let mut draw = [0.; 16];
    draw[0] = 1.;
    draw[5] = 1.;
    draw[10] = 1.;
    draw[15] = 1.;
    draw[12] = 0.3;
    let projection = {
        let mut m = [0.; 16];
        m[0] = 1.;
        m[5] = 1.;
        m[10] = 1.;
        m[15] = 1.;
        m
    };
    let b = screen_bounds(
        ModelBounds {
            min: [0, 0, 0],
            max: [0, 10, 0],
        },
        1,
        draw,
        projection,
        [100., 100., 100., 100.],
    );
    assert_eq!(b.a, [130, 100]);
    assert_eq!(b.b, [130, 1100]);
    assert_eq!(b.radius, 35);
}

#[test]
fn pick_corpus_matches_all_54_rows() {
    let text = include_str!("../../fixtures/pick-corpus.jsonl");
    let boxes = [
        [-10, 10, -10, 10, 10, 20],
        [-1, 2, -1, 2, 10, 20],
        [-10, 10, -10, 10, -20, -10],
        [-10, 10, -10, 10, 0, 0],
    ];
    let mice = [[400, 300], [399, 299], [410, 300], [0, 0], [800, 600]];
    let caps = [
        [0, 0, 10, 0, 5],
        [0, 0, 10, 0, 0],
        [0, 0, 10, 0, 1],
        [i32::MAX, i32::MAX, i32::MAX, i32::MAX, 100],
        [0, 0, 65536, 0, 5],
    ];
    let pts = [
        [0, 0],
        [5, 0],
        [10, 0],
        [11, 0],
        [0, 5],
        [i32::MAX, i32::MAX],
    ];
    let mut seen = 0;
    for line in text.lines() {
        let hit = line.contains("\"hit\":true");
        if line.contains("\"kind\":\"model\"") {
            let b = line
                .split("\"box\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let x = line
                .split("\"x\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<i32>()
                .unwrap();
            let y = line
                .split("\"y\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<i32>()
                .unwrap();
            let q = boxes[b];
            let p = PickablePlayer {
                id: PlayerPickId {
                    pid: 1,
                    generation: 1,
                },
                bounds: pick_bounds(
                    ModelBounds {
                        min: [q[0], q[2], q[4]],
                        max: [q[1], q[3], q[5]],
                    },
                    0,
                ),
                screen_bounds: None,
                projected_depth: 0,
                matrix: [
                    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
                ],
                active: true,
            };
            let got = !pick_players(&[p], [400., 300., 400., 300.], [x, y], [0, 0]).is_empty();
            assert_eq!(got, hit, "model row {b} {x},{y}");
        } else {
            let c = line
                .split("\"capsule\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<usize>()
                .unwrap();
            let x = line
                .split("\"x\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<i64>()
                .unwrap() as i32;
            let y = line
                .split("\"y\":")
                .nth(1)
                .and_then(|s| s.split(',').next())
                .unwrap()
                .parse::<i64>()
                .unwrap() as i32;
            let q = caps[c];
            assert_eq!(
                capsule_hit(
                    ScreenBounds {
                        a: [q[0], q[1]],
                        b: [q[2], q[3]],
                        radius: q[4],
                        enabled: true
                    },
                    [x, y],
                    [0, 0]
                ),
                hit,
                "capsule row {c} {x},{y}"
            );
        }
        seen += 1;
    }
    assert_eq!(seen, 54);
    let _ = (mice, pts);
}
