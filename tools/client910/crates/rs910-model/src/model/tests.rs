use super::*;

/// Hand-built minimal `ModelUnlit` blob: 1 quad (4 verts, 2 faces).
///
/// Layout (version 12, no optional sections; section lengths x=4, y=3, z=1,
/// face index=4, rest 0):
/// verts (0,0,0) (64,0,0) (64,64,0) (0,64,0); faces (0,1,2) via opcode 1
/// and (0,2,3) via opcode 2; both faces colour `0x1020`. Delta streams
/// only carry entries for vertices with the matching flag bit set: X for
/// v0,v1,v3; Y for v0,v2; Z for v0.
fn synthetic_quad_bytes() -> Vec<u8> {
    let mut data = vec![
        1, 0, 12, // tag, unused, version
        7, 1, 2, 1, // vertex flags: XYZ, X, Y, X
        1, 2, // face opcodes: fresh tri, reuse
        64, 65, 65, 65, // face-index deltas: 0,1,1 then 1
        0x10, 0x20, 0x10, 0x20, // face colours: 0x1020 x2
        0x40, 0xC0, 0x40, 0x00, // X deltas (v0,v1,v3): 0, 64, -64
        0x40, 0xC0, 0x40, // Y deltas (v0,v2): 0, 64
        0x40, // Z deltas (v0): 0
    ];
    // Footer: vertex count, face count, texture triangle count, flags, the
    // priority/translucency/label/material bytes, then the section lengths.
    data.extend_from_slice(&[
        0x00, 0x04, // vertexCount = 4
        0x00, 0x02, // faceCount = 2
        0x00, 0x00, // texTriCount = 0
        0x00, // flags: no optional sections
        0x00, // default priority 0
        0x00, // no translucency
        0x00, // no face labels
        0x00, // no materials
        0x00, // no vertex labels
        0x00, 0x04, // X section = 4 bytes
        0x00, 0x03, // Y section = 3 bytes
        0x00, 0x01, // Z section = 1 byte
        0x00, 0x04, // face-index section = 4 bytes
        0x00, 0x00, // no mappings
        0x00, 0x00, // no vertex labels
        0x00, 0x00, // no face labels
    ]);
    data
}

#[test]
fn synthetic_quad_decodes_wire_exact() {
    let raw = decode(&synthetic_quad_bytes()).unwrap();
    assert_eq!(raw.version, 12);
    assert_eq!(
        raw.verts,
        vec![
            [0.0, 0.0, 0.0],
            [64.0, 0.0, 0.0],
            [64.0, 64.0, 0.0],
            [0.0, 64.0, 0.0],
        ]
    );
    assert_eq!(raw.faces, vec![[0, 1, 2], [0, 2, 3]]);
    // 0x1020 -> hue (>>10)&63 = 4, sat (>>7)&7 = 0, light &127 = 32.
    assert_eq!(raw.colors, vec![[4, 0, 32], [4, 0, 32]]);
    assert_eq!(raw.materials, vec![-1, -1]);
    assert_eq!(raw.priorities, vec![0, 0]);
    assert_eq!(raw.alphas, vec![0, 0]);
    assert_eq!(raw.face_types, vec![0, 0]);
    assert!(raw.tex_tris.is_empty());
}

#[test]
fn footer_errors_are_err_not_panic() {
    let good = synthetic_quad_bytes();
    assert!(decode(&[]).is_err());
    assert!(decode(&[1]).is_err());
    assert!(decode(&good[..10]).is_err());
    assert!(decode(&good[..good.len() - 1]).is_err());
    // Bad tag byte (1 is expected).
    let mut bad_tag = good.clone();
    bad_tag[0] = 2;
    assert!(decode(&bad_tag).is_err());
    assert!(crate::modelunlit::ModelUnlit::decode(&[2; 40]).is_err());
    // Bad face-index opcode (only 1-4 legal).
    let mut bad_op = good.clone();
    bad_op[7] = 7;
    assert!(decode(&bad_op).is_err());
    // Absurd footer counts.
    let mut bad_count = good.clone();
    let footer = bad_count.len() - 26;
    bad_count[footer] = 0xFF;
    bad_count[footer + 1] = 0xFF;
    assert!(decode(&bad_count).is_err());
}

// Orientation on real asymmetric models (hair 230 above legs 40250,
// boots 181 feet at origin, door 87426 upright). Run with
// `cargo test real_pack_orientation_upright -- --ignored --nocapture`.

// Slow: touches the real model packs on disk. Run explicitly with
// `cargo test -p client910 -- --ignored` (from tools/client910/).

// --- Multi-model merge ---

// --- skinning ---

// --- corner_uvs ---

/// One textured triangle over wire verts, caller-chosen tri +
/// mapping. Faces reference verts [0, 1, 2]; materials [5].
fn textured_tri(
    verts: [[f32; 3]; 3],
    tri: TexTri,
    mapping: i32,
    tex: TexData,
) -> (RawModel, TexData) {
    let raw = RawModel {
        version: 14,
        verts: verts.to_vec(),
        faces: vec![[0, 1, 2]],
        colors: vec![[4, 0, 32]],
        materials: vec![5],
        priorities: vec![0],
        alphas: vec![0],
        face_types: vec![0],
        tex_tris: vec![tri],
    };
    let mut full_tex = tex;
    full_tex.face_mappings = Some(vec![mapping]);
    if full_tex.face_tex_offsets.is_empty() {
        full_tex.face_tex_offsets = vec![None];
    }
    (raw, full_tex)
}

fn assert_uv_close(got: &[[f32; 2]; 3], want: &[[f32; 2]; 3], tol: f32) {
    for (g, w) in got.iter().zip(want.iter()) {
        for (gc, wc) in g.iter().zip(w.iter()) {
            assert!(
                (gc - wc).abs() <= tol,
                "got {got:?}, want {want:?} (tol {tol})"
            );
        }
    }
}

#[test]
fn corner_uvs_direct_samples_uv_stream() {
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
        TexTri {
            kind: 0,
            verts: [0, 0, 0],
            scale: None,
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        32766,
        TexData {
            vert_counts: vec![1, 1, 1],
            uvs: vec![[4096, 0], [0, 4096], [2048, 2048]],
            face_mappings: None,
            face_tex_offsets: vec![Some([0, 0, 0])],
        },
    );
    // g2s / 4096: (4096,0)->(1,0), (0,4096)->(0,1), (2048,2048)->(0.5,0.5).
    assert_eq!(
        tex.corner_uvs(&raw, 0),
        Some([[1.0, 0.0], [0.0, 1.0], [0.5, 0.5]])
    );
    // Per-face offsets index INTO the owned pairs: counts [2,0,0] +
    // offset 1 on corner 0 samples uvs[1]; corners 1-2 run past the end
    // (base 2 + 0, only 2 pairs) -> whole face is None (caller averages).
    let tex_off = TexData {
        vert_counts: vec![2, 0, 0],
        uvs: vec![[100, 200], [4096, 4096]],
        face_mappings: Some(vec![32766]),
        face_tex_offsets: vec![Some([1, 0, 0])],
    };
    assert_eq!(tex_off.corner_uvs(&raw, 0), None);
    // Untextured face (material -1) takes the colour branch -> None.
    let mut raw_u = raw.clone();
    raw_u.materials = vec![-1];
    assert_eq!(tex.corner_uvs(&raw_u, 0), None);
    // Face out of range -> None, never panic.
    assert_eq!(tex.corner_uvs(&raw, 7), None);
}

#[test]
fn corner_uvs_default_triangle() {
    // Mapping -1 pins (0,1),(1,1),(0,0).
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 0.0, 10.0]],
        TexTri {
            kind: 0,
            verts: [0, 0, 0],
            scale: None,
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        -1,
        TexData::default(),
    );
    assert_eq!(
        tex.corner_uvs(&raw, 0),
        Some([[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]])
    );
}

#[test]
fn corner_uvs_kind0_barycentric() {
    // Hand work: tri == face (q1=(0,0,0), q2=(100,0,0), q3=(0,100,0)).
    // qu=(100,0,0), qv=(0,100,0); n = qu x qv = (0,0,1e8... (0,0,10000)).
    // row_u = qv x n = (1e6,0,0), det_u = qu.row_u = 1e8:
    //   p1=(0,0,0) -> 0; p2=(100,0,0) -> 100*1e6/1e8 = 1; p3 -> 0.
    // row_v = qu x n = (0,-1e6,0), det_v = qv.row_v = -1e8:
    //   p1 -> 0; p2 -> 0; p3=(0,100,0) -> 100*-1e6/-1e8 = 1.
    // So the identity tri pins (0,0),(1,0),(0,1) — homogeneous, so the
    // x100 scale vs the unit derivation changes nothing.
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
        TexTri {
            kind: 0,
            verts: [0, 1, 2],
            scale: None,
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    let got = tex.corner_uvs(&raw, 0).expect("kind 0 resolves");
    assert_uv_close(&got, &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 1e-4);
}

#[test]
fn corner_uvs_kind1_cylindrical() {
    // Hand work (cylindrical mapping):
    // face (0,0,0),(100,0,0),(0,0,100); tri dir (0,0,0) + angle 0 ->
    // identity matrix (the zero-direction arm); kind-1 scales
    // (1024,64,1024) -> rows x(1,1), wrap scaleZ/1024 = 1.
    // Centre = ((0+100)/2, 0, (0+100)/2) = (50,0,50).
    // p1 d=(-50,0,-50): atan2(-50,-50) = -3pi/4 -> -0.375+0.5 = 0.125,
    //   V = 0+0.5+0 = 0.5. p2 d=(50,0,-50): atan2 = 3pi/4 -> 0.875.
    // p3 d=(-50,0,50): atan2 = -pi/4 -> 0.375.
    // Seam fix (dir bit0 == 0, wrap 0.5): 0.875-0.125 = 0.75 > 0.5 ->
    //   corner2 U -= 1 -> -0.125; corner3 gap 0.25 stays.
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        TexTri {
            kind: 1,
            verts: [0, 0, 0],
            scale: Some([1024, 64, 1024]),
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    let got = tex.corner_uvs(&raw, 0).expect("kind 1 resolves");
    assert_uv_close(&got, &[[0.125, 0.5], [-0.125, 0.5], [0.375, 0.5]], 1e-4);
}

#[test]
fn corner_uvs_kind1_version12_shifts_scales() {
    // Same face as above at version 12: verts x4, scales <<2 EXCEPT kind-1
    // Z (the power-of-two rescale) -> scales (4096,256,1024).
    // Kind-1 matrix rows: (1, 64/256 = 0.25, 4096/1024 = 4); wrap = 1.
    // Centre = (200,0,200). p1 d=(-200,0,-200): tx=-200, tz=-800:
    //   atan2(-200,-800) = -(pi-atan(0.25)) = -2.8966140 -> /2pi+0.5 =
    //   0.0389945; V = 0+0.5 = 0.5.
    // p2 d=(200,0,-200): atan2(200,-800) = 2.8966140 -> 0.9610055.
    // p3 d=(-200,0,200): atan2(-200,800) = -0.2449787 -> 0.4610055.
    // Seam: 0.9610055-0.0389945 = 0.922011 > 0.5 -> corner2 -= 1.
    let (mut raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        TexTri {
            kind: 1,
            verts: [0, 0, 0],
            scale: Some([1024, 64, 1024]),
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    raw.version = 12;
    let got = tex.corner_uvs(&raw, 0).expect("kind 1 v12 resolves");
    assert_uv_close(
        &got,
        &[[0.0389945, 0.5], [-0.0389945, 0.5], [0.4610055, 0.5]],
        1e-3,
    );
}

#[test]
fn corner_uvs_kind1_direction_wraps_signed() {
    // The decoder stores tri direction params as SIGNED shorts: raw g2
    // `0xFFFF` is -1, not 65535. Unwrapped, `f1 = 65535/32767 ≈ 2.0`
    // makes `1 - f1*f1` negative and NaNs the mapping matrix (whole
    // face falls back); wrapped, `f1 ≈ 0` and the face resolves finite
    // (exact).
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        TexTri {
            kind: 1,
            verts: [0, 0xFFFF, 0],
            scale: Some([1024, 64, 1024]),
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    let got = tex.corner_uvs(&raw, 0).expect("signed direction resolves");
    assert!(got.iter().flatten().all(|c| c.is_finite()));
}

#[test]
fn corner_uvs_kind2_planar() {
    // Hand work (planar mapping):
    // flat face (0,0,0),(100,0,0),(0,0,100); scales (64,64,64) ->
    // identity matrix; trans (0,0), speed 0.
    // Edges e1=(100,0,0), e2=(0,0,100); n = e1 x e2 = (0,-10000,0).
    // Plane = dominant axis of (0,-10000,0): |ny| dominant, ny < 0 -> 1.
    // Centre (50,0,50). Plane-1 arms: U = dx+0.5, V = dz+0.5:
    //   p1 (-50,-50) -> (-49.5,-49.5); p2 (50,-50) -> (50.5,-49.5);
    //   p3 (-50,50) -> (-49.5,50.5). No seam fixups on kind 2.
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        TexTri {
            kind: 2,
            verts: [0, 0, 0],
            scale: Some([64, 64, 64]),
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: Some([0, 0]),
        },
        0,
        TexData::default(),
    );
    let got = tex.corner_uvs(&raw, 0).expect("kind 2 resolves");
    assert_uv_close(&got, &[[-49.5, -49.5], [50.5, -49.5], [-49.5, 50.5]], 1e-4);
}

#[test]
fn corner_uvs_kind3_spherical() {
    // Hand work (spherical mapping):
    // face (0,0,0),(100,0,0),(0,100,0); scales (1024,1024,1024) + zero
    // direction tri -> identity matrix. Centre = (50,50,0).
    // p1 d=(-50,-50,0), r = sqrt(5000) ~= 70.7107:
    //   U = atan2(-50,0)/2pi+0.5 = -0.25+0.5 = 0.25;
    //   V = asin(-50/70.7107)/pi+0.5 = asin(-0.7071)/pi+0.5 = -0.25+0.5.
    // p2 d=(50,-50,0): U = 0.75, V = 0.25. p3 d=(-50,50,0): U = 0.25,
    //   V = 0.75. Seam gaps are exactly 0.5 (not > 0.5) -> no fixup.
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
        TexTri {
            kind: 3,
            verts: [0, 0, 0],
            scale: Some([1024, 1024, 1024]),
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    let got = tex.corner_uvs(&raw, 0).expect("kind 3 resolves");
    assert_uv_close(&got, &[[0.25, 0.25], [0.75, 0.25], [0.25, 0.75]], 1e-4);
}

#[test]
fn corner_uvs_degenerate_reports_fallback() {
    // Zero-area kind-0 tri (1/0 determinants) -> non-finite -> None, never
    // NaN-poisoned output, never panic. Kinds genuinely unportable (bad
    // tri index, kind 1-3 without scale payload) report None too.
    let (raw, tex) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
        TexTri {
            kind: 0,
            verts: [0, 0, 0],
            scale: None,
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    assert_eq!(tex.corner_uvs(&raw, 0), None);
    let (raw_bad, tex_bad) = textured_tri(
        [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 100.0]],
        TexTri {
            kind: 2,
            verts: [0, 0, 0],
            scale: None,
            rotation: 0,
            direction: 0,
            speed: 0,
            trans: None,
        },
        0,
        TexData::default(),
    );
    assert_eq!(tex_bad.corner_uvs(&raw_bad, 0), None);
    let tex_oob = TexData {
        vert_counts: Vec::new(),
        uvs: Vec::new(),
        face_mappings: Some(vec![9]),
        face_tex_offsets: vec![None],
    };
    assert_eq!(tex_oob.corner_uvs(&raw, 0), None);
}

// Real-pack census: ignored by default (needs server/data/pack on disk).
// Run with `cargo test real_pack_loc_uv_census -- --ignored --nocapture`.

// --- Hillchange ---
