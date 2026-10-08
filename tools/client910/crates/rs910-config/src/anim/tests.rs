use super::*;

/// Hand-built `AnimBase` blob, worked in the test below:
/// 2 ops (`[1, 6→2]`), opaque `[true, false]`, priorities `[7, 8]`,
/// skins `[[5, 6], [7]]` (lengths grouped BEFORE entries: lengths
/// `[2, 1]`, then entries `5, 6, 7`;
/// an interleaved per-op layout would read `[2, 5]` as the lengths),
/// 1 joint (parent `-1`, 1 matrix: identity 16 floats + offset
/// `[1.0, 2.0, 3.0]`), skin-order `[9]`.
fn synthetic_base_bytes() -> Vec<u8> {
    let mut data = Vec::new();
    // op count = 2.
    data.extend_from_slice(&[0, 2]);
    // op types: 1 (translate), 6 (remapped to 2 = rotate).
    data.extend_from_slice(&[1, 6]);
    // opaque flags: op0 yes, op1 no.
    data.extend_from_slice(&[1, 0]);
    // priorities: 7, 8.
    data.extend_from_slice(&[0, 7, 0, 8]);
    // skins: lengths [2, 1] grouped, then entries [5, 6] + [7].
    data.extend_from_slice(&[2, 1, 5, 6, 7]);
    // joint count = 1, matrix count = 1.
    data.extend_from_slice(&[0, 1, 1]);
    // joint 0: parent -1 (0xFFFF), identity matrix, offset (1,2,3).
    data.extend_from_slice(&[0xFF, 0xFF]);
    for i in 0..16 {
        let v: f32 = if i % 5 == 0 { 1.0 } else { 0.0 };
        data.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    for v in [1.0_f32, 2.0, 3.0] {
        data.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    // skin-order: [9].
    data.extend_from_slice(&[0, 1, 0, 9]);
    data
}

#[test]
fn base_decodes_fields_wire_exact() {
    let base = decode_base(42, &synthetic_base_bytes()).unwrap();
    assert_eq!(base.id, 42);
    // A stored 6 is decoded as 2.
    assert_eq!(base.op_types, vec![1, 2]);
    assert_eq!(base.opaque, vec![true, false]);
    assert_eq!(base.priorities, vec![7, 8]);
    assert_eq!(base.skins, vec![vec![5, 6], vec![7]]);
    assert_eq!(base.joints.len(), 1);
    assert_eq!(base.joints[0].parent, -1);
    assert_eq!(base.joints[0].parent_link, None, "root has no link");
    let mut identity = [0.0_f32; 16];
    for (i, e) in identity.iter_mut().enumerate() {
        *e = if i % 5 == 0 { 1.0 } else { 0.0 };
    }
    assert_eq!(base.joints[0].matrices, vec![identity]);
    assert_eq!(base.joints[0].offsets, vec![[1.0, 2.0, 3.0]]);
    assert_eq!(base.skin_order, vec![9]);
}

#[test]
fn base_parent_links_resolve() {
    // 0 ops, 2 joints: joint 0 root, joint 1 child of 0 (matrix count 0
    // keeps the blob tiny — no matrix bytes follow).
    let mut data = vec![0, 0, 0, 2, 0]; // op count 0, joint count 2, matrices 0.
    data.extend_from_slice(&[0xFF, 0xFF]); // joint 0 parent -1.
    data.extend_from_slice(&[0, 0]); // joint 1 parent 0.
    data.extend_from_slice(&[0, 0]); // skin-order: empty.
    let base = decode_base(7, &data).unwrap();
    assert_eq!(base.joints[0].parent_link, None);
    assert_eq!(base.joints[1].parent_link, Some(0));
}

#[test]
fn base_truncation_and_corruption_are_errors() {
    assert!(decode_base(1, &[]).is_err());
    assert!(decode_base(1, &[0]).is_err());
    // Op count exceeds the blob.
    assert!(decode_base(1, &[0, 9, 1]).is_err());
    let err = decode_base(1, &[0, 9, 1]).unwrap_err();
    assert!(err.to_string().contains("op count 9"), "{err}");
}

/// Hand-built `AnimFrame` for the synthetic base above (version 2):
/// - op 0 (type 1 = translate): flag `0b0000_0011` — x+y set, blend 0.
///   values: x smart `70` (1-byte: 70-64 = 6), y smart `0xC000|...`
///   two-byte `0xC010` = 0xC010-0xC000 = 16.
/// - op 1 (type 2 = rotate): flag `0b0001_0100` — z set, blend 2
///   (`0b10` in bits 3-4). values: z smart `80` → 80-64 = 16, then
///   `<< 2 & 0x3FFF` = 64.
///   Pivot: op 1 is type 2 with last type-0 op = none → pivot -1.
///   (No type-0 op exists in this base, so `last_zero` stays -1.)
fn synthetic_frame_bytes() -> Vec<u8> {
    vec![
        2, // version (>= 2, but no type-7 op here so no stride reads)
        0,
        42, // base id 42 (header word, skipped by decode_frame)
        0,
        2,           // op count = 2
        0b0000_0011, // op 0 flags: x + y
        0b0001_0100, // op 1 flags: z + blend 2
        70,          // op 0 x = 6
        0xC0,
        0x10, // op 0 y = 16
        80,   // op 1 z raw 16 → rotated 64
    ]
}

#[test]
fn frame_decodes_ops_wire_exact() {
    let base = decode_base(42, &synthetic_base_bytes()).unwrap();
    let frame = decode_frame(3, 42, &base, &synthetic_frame_bytes()).unwrap();
    assert_eq!(frame.base_id, 42);
    assert_eq!(frame.version, 2);
    assert_eq!(frame.ops.len(), 2);
    assert_eq!(
        frame.ops[0],
        FrameOp {
            skin: 0,
            x: 6,
            y: 16,
            z: 0,
            pivot: -1,
            blend: 0,
        }
    );
    assert_eq!(
        frame.ops[1],
        FrameOp {
            skin: 1,
            x: 0,
            y: 0,
            z: 64,
            pivot: -1,
            blend: 2,
        }
    );
    assert!(!frame.has_alpha_op && !frame.has_colour_op && !frame.has_billboard_op);
}

#[test]
fn frame_v2_type7_double_smart_and_pivot() {
    // Base: op 0 type 0 (pivot), op 1 type 7 (colour), op 2 type 1.
    // Layout: op count g2, 3 type bytes, 3 opaque bytes, 3x g2
    // priorities, 3 empty skin lists, joint count g2 + matrix count g1,
    // empty skin-order g2s.
    let mut base_data = vec![0, 3, 0, 7, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    base_data.extend_from_slice(&[0, 0, 0]); // skins: three empty lists.
    base_data.extend_from_slice(&[0, 0, 0]); // joints 0, matrices 0.
    base_data.extend_from_slice(&[0, 0]); // skin-order empty.
    let base = decode_base(9, &base_data).unwrap();
    assert_eq!(base.op_types, vec![0, 7, 1]);
    // Frame: all three ops posed. op 0 (type 0): flag x only → x = 4.
    // op 1 (type 7, v2): flags x+y → each value followed by a stride
    // smart (consumed + discarded): x = 10 (stride 0), y = 0 (stride 0).
    // op 2 (type 1): flag z → z = 1; explicit op 0 already set the pivot.
    let data = vec![
        2,
        0,
        9,
        0,
        3,           // version, base id, count
        0b0000_0001, // op 0: x
        0b0000_0011, // op 1: x + y
        0b0000_0100, // op 2: z
        68,          // op 0 x = 4
        74,
        64, // op 1 x = 10, stride 0
        64,
        64, // op 1 y = 0, stride 0
        65, // op 2 z = 1
    ];
    // Correction: op 1 flag has y bit set, so y reads value+stride.
    // y value smart 64 → 0, stride 64 → 0. has_colour_op = true.
    let frame = decode_frame(0, 9, &base, &data).unwrap();
    assert_eq!(frame.ops.len(), 3);
    assert_eq!(frame.ops[0].x, 4);
    assert_eq!((frame.ops[1].x, frame.ops[1].y, frame.ops[1].z), (10, 0, 0));
    assert!(frame.has_colour_op);
    // The explicit pivot is not inserted again.
    assert_eq!(frame.ops[2].pivot, -1);
    assert_eq!(frame.ops[2].z, 1);
}

#[test]
fn frame_flag_classes_set_render_bits() {
    // Base: types [5, 7, 9]. Each op posed with x only. Same layout as
    // above: 2 + 3 + 3 + 6 + 3 + 3 + 2 bytes.
    let mut base_data = vec![0, 3, 5, 7, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    base_data.extend_from_slice(&[0, 0, 0]);
    base_data.extend_from_slice(&[0, 0, 0]);
    base_data.extend_from_slice(&[0, 0]);
    let base = decode_base(5, &base_data).unwrap();
    let data = vec![
        1, 0, 5, 0, 3, // v1 (single-smart reads), base, count
        1, 1, 1, // flags: x on each
        65, 66, 67, // x = 1, 2, 3
    ];
    let frame = decode_frame(0, 5, &base, &data).unwrap();
    assert!(frame.has_alpha_op && frame.has_colour_op && frame.has_billboard_op);
}

#[test]
fn frame_corruption_names_id() {
    let base = decode_base(42, &synthetic_base_bytes()).unwrap();
    // Op count exceeds the base (the original yields an empty frame; we name it).
    let mut bad = vec![2, 0, 42, 0, 5, 1, 1, 1, 1, 1];
    let err = decode_frame(3, 42, &base, &bad).unwrap_err();
    assert!(err.to_string().contains("frame 3"), "{err}");
    // Trailing bytes after the value stream.
    bad = synthetic_frame_bytes();
    bad.push(0);
    let err = decode_frame(3, 42, &base, &bad).unwrap_err();
    assert!(err.to_string().contains("frame 3"), "{err}");
    assert!(err.to_string().contains("trailing"), "{err}");
    // Truncated value stream.
    let short = &synthetic_frame_bytes()[..7];
    assert!(decode_frame(3, 42, &base, short).is_err());
    assert!(decode_frame(3, 42, &base, &[]).is_err());
}

#[test]
fn split_frame_id_matches_seq_layout() {
    // Low word first, then the high word shifted by 16.
    assert_eq!(split_frame_id(0x0012_0003), (0x12, 3));
    assert_eq!(split_frame_id(18019), (0, 18019));
}

/// Hand-built keyframeset body: start 0, end 10, loop 1, one entry —
/// transform type 2 (3 slots), skin smart `64` (= 0), component 2
/// (slot 1), curve (1 keyframe: time 5, value 1.5, tangents 0/0/1/1),
/// pre/post 0, linear (bezier 0).
fn synthetic_keyframe_body() -> Vec<u8> {
    let mut data = vec![
        0, 0, // start
        0, 10, // end
        1,  // loop point
        0, 1,  // one entry
        2,  // transform type 2
        64, // skin 0 (single-byte smart)
        2,  // component 2 → slot 1
        0, 1, // one keyframe
        7, // curve type (retained raw)
        0, // pre CONSTANT
        0, // post CONSTANT
        0, // bezier off
        0, 5, // time = 5
    ];
    data.extend_from_slice(&1.5_f32.to_bits().to_be_bytes());
    for v in [0.0_f32, 0.0, 1.0, 1.0] {
        data.extend_from_slice(&v.to_bits().to_be_bytes());
    }
    data
}

#[test]
fn keyframeset_decodes_curves_wire_exact() {
    let base = decode_base(42, &synthetic_base_bytes()).unwrap();
    let set = decode_keyframeset_body(11, &base, 1, &synthetic_keyframe_body()).unwrap();
    assert_eq!(set.id, 11);
    assert_eq!((set.start, set.end, set.loop_point), (0, 10, 1));
    assert_eq!(set.base_id, 42);
    let entry = set.curves[0].as_ref().expect("op 0 curves");
    assert_eq!(entry.len(), 3, "transform type 2 has 3 slots");
    assert!(entry[0].is_none() && entry[2].is_none());
    let curve = entry[1].as_ref().expect("component-2 curve");
    assert_eq!(
        (curve.curve_type, curve.pre, curve.post, curve.bezier),
        (7, 0, 0, false)
    );
    assert_eq!(
        curve.keyframes,
        vec![KeyFrame {
            time: 5,
            value: 1.5,
            tan_in: [0.0, 0.0],
            tan_out: [1.0, 1.0],
        }]
    );
}

#[test]
fn keyframeset_unknown_discriminators_name_id_and_value() {
    let base = decode_base(42, &synthetic_base_bytes()).unwrap();
    // Unknown transform type 9.
    let mut bad = synthetic_keyframe_body();
    bad[7] = 9;
    let err = decode_keyframeset_body(11, &base, 1, &bad).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("11") && msg.contains('9'), "{msg}");
    // Unknown component 17.
    let mut bad = synthetic_keyframe_body();
    bad[9] = 17;
    let err = decode_keyframeset_body(11, &base, 1, &bad).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("11") && msg.contains("17"), "{msg}");
    // Unknown infinity 9.
    let mut bad = synthetic_keyframe_body();
    bad[13] = 9;
    let err = decode_keyframeset_body(11, &base, 1, &bad).unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("11") && msg.contains('9'), "{msg}");
    // Component slot outside the transform's slot count (component 14 →
    // slot 4, but type 2 has 3 slots).
    let mut bad = synthetic_keyframe_body();
    bad[9] = 14;
    let err = decode_keyframeset_body(11, &base, 1, &bad).unwrap_err();
    assert!(err.to_string().contains("11"), "{err}");
    // Skin outside the base.
    let mut bad = synthetic_keyframe_body();
    bad[8] = 70; // smart 70-64 = 6 ≥ 2 base ops.
    let err = decode_keyframeset_body(11, &base, 1, &bad).unwrap_err();
    assert!(err.to_string().contains("11"), "{err}");
}
