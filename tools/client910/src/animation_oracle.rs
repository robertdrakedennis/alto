//! Fixed-input oracle fixtures. Expected states come from the recording of the original client.
use crate::{
    anim::{AnimBase, AnimFrame, FrameOp},
    draw_trace::Trace,
    gpumodel::GpuModel,
};

fn model(seed: i32) -> GpuModel {
    let mut m = GpuModel::new(
        &crate::gpumodel::ModelStores {
            materials: &Default::default(),
            billboards: &Default::default(),
            emitters: &Default::default(),
        },
        &Default::default(),
        crate::gpumodel::BuildParams {
            flags: 0,
            ambient: 64,
            contrast: 850,
            detail: 55,
        },
    )
    .unwrap();
    m.vertex_count_all = 9;
    m.vertex_count = 8;
    m.unique_count = 16;
    m.face_count = 8;
    m.vx = (0..9).map(|i| (i * 137 + seed * 13) % 1024 - 511).collect();
    m.vy = (0..9).map(|i| (i * 293 + seed * 3) % 1024 - 511).collect();
    m.vz = (0..9).map(|i| (i * 31 + seed * 71) % 1024 - 511).collect();
    m.nx = (0..16).map(|i| (i * 4001 + seed * 619) as i16).collect();
    m.ny = (0..16).map(|i| (i * 2003 - seed * 307) as i16).collect();
    m.nz = (0..16).map(|i| (i * 907 + seed * 51) as i16).collect();
    m.vertex_offsets = (0..=8).map(|i| i * 2).collect();
    m.vertex_slots = (1..=16).collect();
    m.vertex_source_models = Some((0..9).map(|i| (1 << (i % 4)) as i16).collect());
    m.face_part = Some((0..8).map(|i| (1 << (i % 4)) as i16).collect());
    m.vertex_groups = Some(vec![vec![0, 3, 6], vec![1, 4, 7], vec![2, 5], vec![]]);
    m.face_groups = m.vertex_groups.clone();
    m.face_alpha = (0..8).map(|i| (i * 37) as i8).collect();
    m.face_colour = (0..8).map(|i| (i * 9811) as i16).collect();
    m
}
fn model_input(m: &GpuModel) -> Vec<i32> {
    let mut v = vec![
        m.vertex_count_all,
        m.vertex_count,
        m.unique_count,
        m.face_count,
    ];
    v.extend(&m.vx);
    v.extend(&m.vy);
    v.extend(&m.vz);
    for a in [
        &m.nx,
        &m.ny,
        &m.nz,
        m.vertex_source_models.as_ref().unwrap(),
        m.face_part.as_ref().unwrap(),
        &m.face_colour,
    ] {
        v.extend(a.iter().map(|&n| n as i32));
    }
    v.extend(m.face_alpha.iter().map(|&n| n as i32));
    v
}
fn result(m: &mut GpuModel) -> Vec<i32> {
    let mut v = Vec::new();
    v.extend(&m.vx);
    v.extend(&m.vy);
    v.extend(&m.vz);
    for a in [&m.nx, &m.ny, &m.nz, &m.face_colour] {
        v.extend(a.iter().map(|&n| n as i32));
    }
    v.extend(m.face_alpha.iter().map(|&n| n as i32));
    v.extend([
        m.min_x(),
        m.max_x(),
        m.min_y(),
        m.max_y(),
        m.min_z(),
        m.max_z(),
        m.horizontal_radius(),
        m.radius(),
    ]);
    v
}
fn frame_input(v: &mut Vec<i32>, f: &AnimFrame) {
    v.push(f.ops.len() as i32);
    for o in &f.ops {
        v.extend([o.skin as i32, o.x, o.y, o.z, o.pivot, o.blend as i32]);
    }
}

fn curve_words(c: &crate::anim::Curve) -> Vec<i32> {
    let mut out = vec![
        c.pre as i32,
        c.post as i32,
        i32::from(c.bezier),
        c.keyframes.len() as i32,
    ];
    for k in &c.keyframes {
        out.push(k.time);
        out.extend(
            [
                k.value,
                k.tan_in[0],
                k.tan_in[1],
                k.tan_out[0],
                k.tan_out[1],
            ]
            .map(|f| f.to_bits() as i32),
        );
    }
    out
}

fn math_fixtures(input: &mut Discard, output: &mut Trace) {
    use crate::anim::{Curve, KeyFrame};
    for pre in 0..5 {
        for post in 0..5 {
            for bezier in [false, true] {
                for mode in 0..6 {
                    let c = Curve {
                        curve_type: 0,
                        pre,
                        post,
                        bezier,
                        keyframes: vec![
                            KeyFrame {
                                time: 3,
                                value: -1.75,
                                tan_in: [3., 1.],
                                tan_out: match mode {
                                    0 => [0., 0.],
                                    1 => [f32::MAX, f32::MAX],
                                    2 => [48., 16.],
                                    3 => [-12., -17.],
                                    _ => [7., 13.],
                                },
                            },
                            KeyFrame {
                                time: 17,
                                value: 11.25,
                                tan_in: if mode == 4 { [-10., 7.] } else { [9., -3.] },
                                tan_out: [9., -5.],
                            },
                            KeyFrame {
                                time: 29,
                                value: -2.5,
                                tan_in: [3., 4.],
                                tan_out: [6., -7.],
                            },
                        ],
                    };
                    let name = format!("curve/{pre}/{post}/{bezier}/{mode}");
                    input.push(&name, curve_words(&c));
                    let evaluated = crate::animation_curve::EvaluatedCurve::new(&c);
                    output.push(
                        name,
                        (-2..=34)
                            .map(|t| evaluated.value(t).to_bits() as i32)
                            .collect(),
                    );
                }
            }
        }
    }
    for seed in 0..32 {
        let angle = [
            seed as f32 * 0.043,
            seed as f32 * -0.072,
            seed as f32 * 0.091,
        ];
        let mut matrix = crate::animation_matrix::rotation(angle[0], angle[1], angle[2]);
        for i in 0..3 {
            matrix[i] *= 1.1;
            matrix[4 + i] *= 0.87;
            matrix[8 + i] *= 1.31;
        }
        matrix[12] = seed as f32 * 13.7;
        matrix[13] = seed as f32 * -8.1;
        matrix[14] = seed as f32 * 2.9;
        let mut words = matrix.map(|v| v.to_bits() as i32).to_vec();
        words.extend(angle.map(|v| v.to_bits() as i32));
        input.push(format!("matrix/{seed}"), words);
        let mut result = Vec::new();
        for value in crate::animation_matrix::inverse(&matrix)
            .into_iter()
            .chain(crate::animation_matrix::euler(&matrix))
            .chain(crate::animation_matrix::rotation(
                angle[0], angle[1], angle[2],
            ))
            .chain(crate::animation_matrix::vector(
                &matrix,
                [1.5, -3.75, 7.25],
                true,
            ))
            .chain(crate::animation_matrix::vector(
                &matrix,
                [1.5, -3.75, 7.25],
                false,
            ))
        {
            result.push(value.to_bits() as i32);
        }
        output.push(format!("matrix/{seed}"), result);
    }
}

fn skeletal_fixtures(input: &mut Discard, output: &mut Trace) {
    use crate::anim::{Curve, Joint, KeyFrame, KeyFrameSet};
    for seed in 0..12 {
        let kinds = vec![0, 1, 2, 3, 5, 7, 0, 1, 2, 3, 0, 2, 5, 7];
        let joints = (0..3)
            .map(|id| {
                let matrices = (0..2)
                    .map(|pose| {
                        let f = (seed + id * 3 + pose * 7) as f32;
                        let mut m =
                            crate::animation_matrix::rotation(f * 0.03, f * -0.013, f * 0.021);
                        m[12] = 13.75 * f;
                        m[13] = -7.31 * f;
                        m[14] = 3.2 * f;
                        m
                    })
                    .collect();
                Joint {
                    parent: id - 1,
                    parent_link: if id == 0 { None } else { Some(id as usize - 1) },
                    matrices,
                    offsets: vec![[0.; 3]; 2],
                }
            })
            .collect();
        let base = AnimBase {
            id: 2,
            op_types: kinds.clone(),
            opaque: vec![false; kinds.len()],
            priorities: (0..kinds.len())
                .map(|i| {
                    if seed % 2 == 0 {
                        65535
                    } else {
                        [1, 3, 5, 15][i % 4]
                    }
                })
                .collect(),
            skins: (0..kinds.len())
                .map(|i| vec![(i % 3) as i32, ((i + 1) % 3) as i32])
                .collect(),
            joints,
            skin_order: vec![0, 2, if seed % 3 == 0 { -1 } else { 1 }],
        };
        let curves = kinds
            .iter()
            .enumerate()
            .map(|(op, &kind)| {
                if kind == 0 || (seed + op as i32) % 7 == 0 {
                    return None;
                }
                Some(
                    (0..if kind == 7 { 6 } else { 3 })
                        .map(|slot| {
                            if (seed + slot) % 4 == 0 {
                                return None;
                            }
                            let a = match kind {
                                1 => 17.3,
                                2 => 0.091,
                                3 => 1.125,
                                5 => 0.07,
                                7 => 7.3,
                                _ => 0.,
                            };
                            let value = a * (slot + 1) as f32;
                            Some(Curve {
                                curve_type: 0,
                                pre: 0,
                                post: 0,
                                bezier: seed % 2 == 0,
                                keyframes: vec![
                                    KeyFrame {
                                        time: 0,
                                        value,
                                        tan_in: [3., a],
                                        tan_out: [9., a],
                                    },
                                    KeyFrame {
                                        time: 20,
                                        value: value + a * 1.3,
                                        tan_in: [6., a],
                                        tan_out: [3., a],
                                    },
                                ],
                            })
                        })
                        .collect(),
                )
            })
            .collect();
        let set = KeyFrameSet {
            id: 2,
            version: 2,
            base_id: 2,
            base,
            start: 0,
            end: 20,
            loop_point: (seed % 2) as u8,
            curves,
            render_flags: 0x180,
        };
        let pose = crate::animation_skeletal::SkeletalPose::new(set.clone()).unwrap();
        for angle in 0..4 {
            for normals in [false, true] {
                for tick in [-1, 0, 7, 19, 21] {
                    let mut m = model(seed);
                    let mut words = model_input(&m);
                    words.extend([angle, i32::from(normals), tick]);
                    words.push(kinds.len() as i32);
                    for (i, &kind) in kinds.iter().enumerate() {
                        words.extend([
                            kind as i32,
                            set.base.priorities[i],
                            set.base.skins[i].len() as i32,
                        ]);
                        words.extend(&set.base.skins[i]);
                    }
                    words.extend([set.loop_point as i32, set.base.joints.len() as i32]);
                    for j in &set.base.joints {
                        words.extend([j.parent, j.matrices.len() as i32]);
                        for m in &j.matrices {
                            words.extend(m.map(|v| v.to_bits() as i32));
                        }
                    }
                    words.push(set.base.skin_order.len() as i32);
                    words.extend(&set.base.skin_order);
                    for row in &set.curves {
                        if let Some(row) = row {
                            words.push(row.len() as i32);
                            for c in row {
                                words.push(i32::from(c.is_some()));
                                if let Some(c) = c {
                                    words.extend(curve_words(c));
                                }
                            }
                        } else {
                            words.push(-1);
                        }
                    }
                    let name = format!("skeletal/{seed}/{angle}/{normals}/{tick}");
                    for part in [1, 3, 65535] {
                        for selected in [false, true] {
                            let mask: Vec<_> = (0..kinds.len())
                                .map(|i| (i as i32 + seed) % 3 == 0)
                                .collect();
                            let mut masked = words.clone();
                            masked.extend([part, selected as i32, mask.len() as i32]);
                            masked.extend(mask.iter().map(|&b| b as i32));
                            let label = format!(
                                "skeletal/masked/{seed}/{angle}/{normals}/{tick}/{part}/{selected}"
                            );
                            input.push(&label, masked);
                            let mut copy = m.clone();
                            copy.apply_animation(&pose.transforms_masked(
                                tick,
                                angle,
                                normals,
                                Some((&mask, selected)),
                                part,
                            ));
                            output.push(label, result(&mut copy));
                        }
                    }
                    input.push(&name, words);
                    m.apply_animation(&pose.transforms(tick, angle, normals));
                    output.push(name, result(&mut m));
                }
            }
        }
    }
}

fn real_fixtures(input: &mut Discard, output: &mut Trace) {
    use crate::{anim, cache::Pack};
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let bytes = |v: &mut Vec<i32>, data: &[u8]| {
        v.push(data.len() as i32);
        v.extend(data.iter().map(|&b| b as i32));
    };
    let index = pack.read_archive_index(anim::ANIMS_ARCHIVE).unwrap();
    for &group in index
        .group_id
        .iter()
        .step_by((index.group_id.len() / 24).max(1))
        .take(24)
    {
        let files = pack.read_group(anim::ANIMS_ARCHIVE, group).unwrap();
        let (&id, a) = files.iter().next().unwrap();
        let bid = anim::frame_base_id(a).unwrap();
        let raw = anim::fetch_file(&pack, anim::BASES_ARCHIVE, bid).unwrap();
        let base = anim::decode_base(bid, &raw).unwrap();
        let frame = anim::decode_frame(id, bid, &base, a).unwrap();
        let (&idb, b) = files
            .iter()
            .rfind(|(_, b)| anim::frame_base_id(b).unwrap() == bid)
            .unwrap();
        let next = anim::decode_frame(idb, bid, &base, b).unwrap();
        for angle in 0..4 {
            for tick in [0, 1, 7, 11] {
                let mut m = model(group as i32 % 32);
                let mut words = model_input(&m);
                words.extend([angle, 1, tick, 12, bid as i32]);
                bytes(&mut words, &raw);
                bytes(&mut words, a);
                bytes(&mut words, b);
                let name = format!("cache-classic/{group}/{angle}/{tick}");
                input.push(&name, words);
                m.apply_animation(&crate::gpumodel::classic_transforms(
                    crate::gpumodel::ClassicPose {
                        base: &base,
                        frame: &frame,
                        next: Some(&next),
                        tick,
                        duration: 12,
                    },
                    angle,
                    true,
                ));
                output.push(name, result(&mut m));
            }
        }
    }
    let index = pack
        .read_archive_index(anim::ANIMS_KEYFRAMES_ARCHIVE)
        .unwrap();
    for &group in index
        .group_id
        .iter()
        .step_by((index.group_id.len() / 24).max(1))
        .take(24)
    {
        let files = pack
            .read_group(anim::ANIMS_KEYFRAMES_ARCHIVE, group)
            .unwrap();
        for (&file, b) in files.iter().take(1) {
            let id = if index.group_id.len() == 1 {
                file
            } else {
                group
            };
            let bid = ((b[1] as u32) << 8) | b[2] as u32;
            let raw = anim::fetch_file(&pack, anim::BASES_ARCHIVE, bid).unwrap();
            let base = anim::decode_base(bid, &raw).unwrap();
            let set = anim::decode_keyframeset_body(id, &base, b[0], &b[3..]).unwrap();
            let start = set.start as i32;
            let end = set.end as i32;
            let pose = crate::animation_skeletal::SkeletalPose::new(set).unwrap();
            for angle in 0..4 {
                for tick in [-1, start, (start + end) / 2, end] {
                    let mut m = model(id as i32 % 32);
                    let mut words = model_input(&m);
                    words.extend([angle, 1, tick, bid as i32]);
                    bytes(&mut words, &raw);
                    bytes(&mut words, b);
                    let name = format!("cache-skeletal/{id}/{angle}/{tick}");
                    input.push(&name, words);
                    m.apply_animation(&pose.transforms(tick, angle, true));
                    output.push(name, result(&mut m));
                }
            }
        }
    }
}

/// The fixtures below name and record every case's input next to its expected
/// output; only the outputs are compared.
struct Discard;
impl Discard {
    fn push(&mut self, _name: impl Into<String>, _words: Vec<i32>) {}
}

/// Synthetic pose and clock fixtures against the frozen recording of the
/// original client's animation code.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn export_animation_oracle() {
    let mut input = Discard;
    let mut output = Trace::default();
    math_fixtures(&mut input, &mut output);
    skeletal_fixtures(&mut input, &mut output);
    real_fixtures(&mut input, &mut output);
    for seed in 0..32 {
        let base = AnimBase {
            id: 1,
            op_types: vec![0, 1, 2, 3, 5, 7, 0, 2],
            opaque: vec![false; 8],
            priorities: (0..8)
                .map(|i| {
                    if seed & 1 == 0 {
                        65535
                    } else {
                        [1, 3, 5, 15][i % 4]
                    }
                })
                .collect(),
            skins: vec![
                vec![0, 1, 2],
                vec![0, 2],
                vec![1, 0],
                vec![2, 1],
                vec![0, 1],
                vec![1, 2],
                vec![3],
                vec![0, 0, 2],
            ],
            joints: vec![],
            skin_order: vec![],
        };
        let frame = |next: bool| AnimFrame {
            base_id: 1,
            version: 2,
            ops: (0..8)
                .filter(|&i| (i + seed + i32::from(next)) % 5 != 0)
                .map(|i| FrameOp {
                    skin: i as usize,
                    x: if i == 2 || i == 7 {
                        (seed * 987 + if next { 16100 } else { 200 }) & 16383
                    } else {
                        seed * 9 - 37
                    },
                    y: if i == 2 || i == 7 {
                        (seed * 761 + 3000) & 16383
                    } else {
                        if next {
                            193
                        } else {
                            125
                        }
                    },
                    z: if i == 2 || i == 7 {
                        (seed * 53 + 16000) & 16383
                    } else {
                        if next {
                            -79
                        } else {
                            131
                        }
                    },
                    pivot: if i == 2 { 0 } else { -1 },
                    blend: if seed % 7 == 0 {
                        if next {
                            1
                        } else {
                            2
                        }
                    } else {
                        0
                    },
                })
                .collect(),
            has_alpha_op: true,
            has_colour_op: true,
            has_billboard_op: false,
        };
        let a = frame(false);
        let b = frame(true);
        for angle in 0..4 {
            for normals in [false, true] {
                for tick in [0, 1, 6, 12] {
                    let mut m = model(seed);
                    let mut values = model_input(&m);
                    values.extend([angle, i32::from(normals), tick, 12]);
                    values.push(base.op_types.len() as i32);
                    for i in 0..base.op_types.len() {
                        values.extend([
                            base.op_types[i] as i32,
                            base.priorities[i],
                            base.skins[i].len() as i32,
                        ]);
                        values.extend(&base.skins[i]);
                    }
                    frame_input(&mut values, &a);
                    frame_input(&mut values, &b);
                    let name = format!("pose/{seed}/{angle}/{normals}/{tick}");
                    for part in [1, 3, 65535] {
                        for selected in [false, true] {
                            let mask: Vec<_> = (0..base.op_types.len())
                                .map(|i| (i as i32 + seed) % 3 == 0)
                                .collect();
                            let mut masked = values.clone();
                            masked.extend([part, selected as i32, mask.len() as i32]);
                            masked.extend(mask.iter().map(|&b| b as i32));
                            let label = format!(
                                "pose/masked/{seed}/{angle}/{normals}/{tick}/{part}/{selected}"
                            );
                            input.push(&label, masked);
                            let mut copy = m.clone();
                            copy.apply_animation(&crate::gpumodel::classic_transforms_masked(
                                crate::gpumodel::ClassicPose {
                                    base: &base,
                                    frame: &a,
                                    next: Some(&b),
                                    tick,
                                    duration: 12,
                                },
                                crate::gpumodel::PoseTarget {
                                    angle,
                                    normals,
                                    blend: Some((&mask, selected)),
                                    part_mask: part,
                                },
                            ));
                            output.push(label, result(&mut copy));
                        }
                    }
                    input.push(&name, values);
                    m.apply_animation(&crate::gpumodel::classic_transforms(
                        crate::gpumodel::ClassicPose {
                            base: &base,
                            frame: &a,
                            next: Some(&b),
                            tick,
                            duration: 12,
                        },
                        angle,
                        normals,
                    ));
                    output.push(name, result(&mut m));
                }
            }
        }
    }
    use crate::{
        animation_playback as playback, entities910::animation_state::Node,
        protocol910::sequence_types::Sequence,
    };
    for kind in 0..3 {
        for mode in 0..3 {
            for random in [false, true] {
                for delay in [0, 7] {
                    for replay in [-1, 2, 4] {
                        let id = 100 + kind;
                        let mut seq = Sequence::empty(id);
                        seq.replayoff = replay;
                        seq.replaycount = 3;
                        seq.tween = kind == 1;
                        if kind == 2 {
                            seq.skeletal = 7;
                        } else {
                            seq.frames = Some(vec![3, 7, 1, 9]);
                            seq.frame_ids = Some(vec![0, 1, 2, 3]);
                        }
                        let mut n = Node::default();
                        let mut rng = playback::AnimationRandom::new(910);
                        let name = format!("clock/{kind}/{mode}/{random}/{delay}/{replay}");
                        let steps = [0, 1, 2, 3, 7, 1, 19, 101, 203];
                        input.push(&name, vec![kind, mode, i32::from(random), delay, replay]);
                        playback::start(&mut n, Some(&seq), delay, mode, random, &mut rng).unwrap();
                        let state = |n: &Node, changed: bool| {
                            vec![
                                n.id(),
                                n.time,
                                n.delay,
                                n.loops,
                                n.frame,
                                n.next,
                                i32::from(n.finished),
                                n.mode,
                                i32::from(n.flag),
                                i32::from(changed),
                            ]
                        };
                        let mut values = state(&n, false);
                        for ticks in steps {
                            if n.finished {
                                playback::start(&mut n, None, 0, 0, false, &mut rng).unwrap();
                            }
                            let changed =
                                playback::advance(&mut n, &seq, ticks, Some((5, 25)), &mut rng);
                            values.extend(state(&n, changed));
                        }
                        output.push(name, values);
                    }
                }
            }
        }
    }
    rs910_core::test_support::frozen::assert_stream("animation/recording", &output.encode());
    eprintln!(
        "[animation] {} synthetic pose/clock sections",
        output.0.len()
    );
}
