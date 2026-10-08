use super::*;
use crate::ui_paint::Op;

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Ops(usize, usize),
    Scene,
    Model(u32),
}

/// The software toolkit's frame loop as it was before `FramePlan::walk`
/// (`sw_toolkit::frame::SoftwareToolkit::render`, Phase 2.8), frozen: the
/// op ranges replayed, the scene draw and the model draws, in order.
fn frozen_loop(
    ops_len: usize,
    scene_op: usize,
    scene: bool,
    models: &[(usize, u32)],
) -> Vec<Event> {
    let mut out = Vec::new();
    let split = scene_op.min(ops_len);
    let mut done = 0;
    let mut stops: Vec<usize> = models.iter().map(|(i, _)| *i).collect();
    stops.push(split);
    stops.sort_unstable();
    stops.dedup();
    for stop in stops {
        let stop = stop.min(ops_len);
        out.push(Event::Ops(done, stop));
        done = stop;
        if stop == split && scene {
            out.push(Event::Scene);
        }
        for (_, draw) in models.iter().filter(|(i, _)| *i == stop) {
            out.push(Event::Model(*draw));
        }
    }
    out.push(Event::Ops(done, ops_len));
    out
}

fn walked(plan: &FramePlan<u32>) -> Vec<Event> {
    let base = plan.recording.ops.as_ptr() as usize;
    let size = std::mem::size_of::<Op>();
    let mut out = Vec::new();
    plan.walk(|segment| match segment {
        Segment::Ops(ops) => {
            // A subslice's pointer (even an empty one's) is its start in the
            // recording.
            let start = (ops.as_ptr() as usize - base) / size;
            out.push(Event::Ops(start, start + ops.len()));
        }
        Segment::Scene(_) => out.push(Event::Scene),
        Segment::Model(m) => out.push(Event::Model(*m)),
    });
    out
}

/// `FramePlan::walk` visits exactly what the software toolkit's frame loop
/// drew, for every small plan: op counts 0..=4, scene op indices past the
/// end, with and without a viewport, and up to three models at any index
/// (including duplicates and indices past the end).
#[test]
fn walk_matches_the_frozen_software_frame_loop() {
    let mut cases = 0;
    for ops_len in 0..=4usize {
        for scene_op in 0..=6usize {
            for scene in [false, true] {
                for count in 0..=3usize {
                    let slots = 7usize.pow(count as u32);
                    for code in 0..slots {
                        let mut c = code;
                        let models: Vec<(usize, u32)> = (0..count)
                            .map(|n| {
                                let index = c % 7;
                                c /= 7;
                                (index, n as u32)
                            })
                            .collect();
                        let plan = FramePlan {
                            recording: Recording {
                                ops: vec![Op::ResetBounds([0; 4]); ops_len],
                                marks: vec![],
                            },
                            scene_op,
                            scene: scene.then_some(Scene {
                                rect: [0; 4],
                                clip: [0; 4],
                            }),
                            models: models.clone(),
                        };
                        assert_eq!(
                            walked(&plan),
                            frozen_loop(ops_len, scene_op, scene, &models),
                            "ops {ops_len} scene_op {scene_op} scene {scene} models {models:?}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 5 * 7 * 2 * (1 + 7 + 49 + 343));
}

/// `from_quads` maps quad boundaries through `Recording::op_index` like the
/// software toolkit's `set_ui` did.
#[test]
fn from_quads_maps_boundaries_through_op_index() {
    let recording = Recording {
        ops: vec![Op::ResetBounds([0; 4]); 5],
        marks: vec![(0, 0), (2, 1), (2, 3), (4, 5)],
    };
    let plan = FramePlan::from_quads(recording.clone(), 2, None, vec![0usize, 4, 9], |q| *q);
    assert_eq!(plan.scene_op, recording.op_index(2));
    assert_eq!(plan.scene_op, 3, "the last mark at quad 2");
    assert_eq!(
        plan.models
            .iter()
            .map(|(i, q)| (*i, *q))
            .collect::<Vec<_>>(),
        [(0, 0), (5, 4), (5, 9)],
        "unmarked quad 9 maps to the op total"
    );
}
