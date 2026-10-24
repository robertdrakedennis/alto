use super::*;

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
