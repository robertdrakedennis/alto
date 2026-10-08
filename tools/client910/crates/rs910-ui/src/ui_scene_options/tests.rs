use super::*;

#[test]
fn invert_round_trips_a_projection() {
    let proj = crate::camera::perspective_fov(50.0, 3000.0, 1.2, 0.9);
    let mut inv = proj;
    invert(&mut inv);
    let id = crate::camera::multiply(&proj, &inv);
    for (i, v) in id.iter().enumerate() {
        let want = if i % 5 == 0 { 1.0 } else { 0.0 };
        assert!((v - want).abs() < 1e-4, "entry {i} = {v}");
    }
}

#[test]
fn fine_height_interpolates_and_zeroes_outside() {
    let mut t = crate::protocol910::terrain::Terrain::new(4, 4).unwrap();
    for x in 0..=4 {
        for z in 0..=4 {
            let i = (x) * 5 + z;
            t.heights[i] = -(x as i32) * 512;
        }
    }
    let h = Heightmap::from_terrain(&t, 1);
    assert_eq!(fine_height(&h, 0, 512, 256), -512);
    assert_eq!(fine_height(&h, 0, 768, 256), -768);
    assert_eq!(fine_height(&h, 0, -1, 0), 0);
    assert_eq!(fine_height(&h, 0, 4 * 512, 0), 0);
}

#[test]
fn move_packet_bytes_follow_wire_order() {
    assert_eq!(
        move_game_click([3136, 3136], [86, 84], true),
        vec![
            33,
            0x0C,
            0x94,
            1,
            ((3222 + 128) & 0xFF) as u8,
            (3222 >> 8) as u8
        ]
    );
}

#[test]
fn minimap_move_packet_bytes_follow_wire_order() {
    // The minimap move: shared head then the anticheat tail. Same walk tile as
    // the game-click vector above (x 3222, z 3220).
    assert_eq!(
        move_minimap_click(
            [3136, 3136],
            [86, 84],
            true,
            0x1234,
            5,
            -3,
            [0x1111, 0x2222]
        ),
        vec![
            crate::proto::client::MOVE_MINIMAPCLICK,
            0x0C,
            0x94,
            1,
            ((3222 + 128) & 0xFF) as u8,
            (3222 >> 8) as u8,
            0xFF,
            0xFF,
            0x12,
            0x34,
            57,
            5,
            253,
            89,
            0x11,
            0x11,
            0x22,
            0x22,
            63,
        ]
    );
}

#[test]
fn face_square_packet_bytes_follow_wire_order() {
    assert_eq!(
        face_square([3136, 3136], [86, 84]),
        vec![crate::proto::client::FACE_SQUARE, 0x0c, 0x14, 0x16, 0x0c]
    );
}

#[test]
fn opnpc1_packet_bytes_follow_wire_order() {
    // `p1_alt3(ctrl)` + `p2_alt2(idx)` for action 9 -> `OPNPC1` (51).
    // `p1_alt3` is `128 - v`; `p2_alt2` is hi, `lo + 128`.
    assert_eq!(build_opnpc(9, 0x1234, false), Some((51, [128, 0x12, 0xB4])));
    assert_eq!(build_opnpc(9, 0x1234, true), Some((51, [127, 0x12, 0xB4])));
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_opnpc(2009, 0x0001, true),
        Some((51, [127, 0x00, 0x81]))
    );
    assert_eq!(
        build_opnpc(10, 1, false),
        Some((crate::proto::client::OPNPC2, [128, 0, 129]))
    );
    assert_eq!(
        build_opnpc(11, 1, false),
        Some((crate::proto::client::OPNPC3, [128, 0, 129]))
    );
    assert_eq!(
        build_opnpc(12, 1, false),
        Some((crate::proto::client::OPNPC4, [128, 0, 129]))
    );
    assert_eq!(
        build_opnpc(13, 1, false),
        Some((crate::proto::client::OPNPC5, [128, 0, 129]))
    );
    assert_eq!(
        build_opnpc(1003, 1, false),
        Some((crate::proto::client::OPNPC6, [128, 0, 129]))
    );
    assert_eq!(build_opnpc(23, 1, false), None);
}

#[test]
fn encode_loc_id_field_layout() {
    // low = x | z<<7 | shape<<14 | angle<<20 |
    // 0x40000000, high = id. x=10, z=20, shape=2, angle=1, id=1234.
    assert_eq!(
        encode_loc_id(1234, 10, 20, 2, 1, false, false),
        ((1234i64) << 32) | 0x4010_8A0A
    );
    // `active == 0` sets the sign bit (bit 63); `raiseobject == 1`
    // sets `1 << 22` (4194304).
    assert_eq!(
        encode_loc_id(1234, 10, 20, 2, 1, true, false) & i64::MIN,
        i64::MIN
    );
    assert_eq!(
        encode_loc_id(1234, 10, 20, 2, 1, false, true) & 4194304,
        4194304
    );
    assert_eq!(
        encode_loc_id(1, 0, 0, 0, 0, false, false) & 0xFFFF_FFFF,
        0x4000_0000
    );
}

#[test]
fn oploc1_packet_bytes_follow_wire_order() {
    // `p1_alt2(ctrl)` + `p2(z)` + `p4(id)` + `p2_alt3(x)` for action 3 ->
    // `OPLOC1` (12). `p1_alt2` is `-v`; `p2_alt3` is `lo + 128`, hi. World
    // x 3210 (0x0C8A), z 3220 (0x0C94); id 1234 (0x000004D2 from the
    // `encode_loc_id` high half).
    let entity = encode_loc_id(1234, 10, 20, 2, 1, false, false);
    assert_eq!(
        build_oploc(3, entity, [3200, 3200], [10, 20], false),
        Some((12, [0, 0x0C, 0x94, 0x00, 0x00, 0x04, 0xD2, 0x0A, 0x0C]))
    );
    assert_eq!(
        build_oploc(3, entity, [3200, 3200], [10, 20], true),
        Some((12, [0xFF, 0x0C, 0x94, 0x00, 0x00, 0x04, 0xD2, 0x0A, 0x0C]))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_oploc(2003, entity, [3200, 3200], [10, 20], true),
        Some((12, [0xFF, 0x0C, 0x94, 0x00, 0x00, 0x04, 0xD2, 0x0A, 0x0C]))
    );
    assert_eq!(
        build_oploc(4, entity, [3200, 3200], [10, 20], false).map(|(op, _)| op),
        Some(crate::proto::client::OPLOC2)
    );
    assert_eq!(
        build_oploc(5, entity, [3200, 3200], [10, 20], false).map(|(op, _)| op),
        Some(crate::proto::client::OPLOC3)
    );
    assert_eq!(
        build_oploc(6, entity, [3200, 3200], [10, 20], false).map(|(op, _)| op),
        Some(crate::proto::client::OPLOC4)
    );
    assert_eq!(
        build_oploc(1001, entity, [3200, 3200], [10, 20], false).map(|(op, _)| op),
        Some(crate::proto::client::OPLOC5)
    );
    assert_eq!(
        build_oploc(1002, entity, [3200, 3200], [10, 20], false).map(|(op, _)| op),
        Some(crate::proto::client::OPLOC6)
    );
    assert_eq!(build_oploc(2, entity, [3200, 3200], [10, 20], false), None);
}

#[test]
fn opobj1_packet_bytes_follow_wire_order() {
    // `p2_alt1(idx)` + `p2_alt1(x)` + `p2(z)` + `p1_alt3(sub + ctrl)` for
    // action 18 -> `OPOBJ1` (81). `p2_alt1` is lo, hi; `p1_alt3` is
    // `128 - v`. World x 3210 (0x0C8A), z 3220 (0x0C94).
    assert_eq!(
        build_opobj(18, 0x1234, [3200, 3200], [10, 20], false, false),
        Some((81, [0x34, 0x12, 0x8A, 0x0C, 0x0C, 0x94, 128]))
    );
    assert_eq!(
        build_opobj(18, 0x1234, [3200, 3200], [10, 20], false, true),
        Some((81, [0x34, 0x12, 0x8A, 0x0C, 0x0C, 0x94, 127]))
    );
    assert_eq!(
        build_opobj(18, 0x1234, [3200, 3200], [10, 20], true, false),
        Some((81, [0x34, 0x12, 0x8A, 0x0C, 0x0C, 0x94, 126]))
    );
    assert_eq!(
        build_opobj(18, 0x1234, [3200, 3200], [10, 20], true, true),
        Some((81, [0x34, 0x12, 0x8A, 0x0C, 0x0C, 0x94, 125]))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_opobj(2018, 1, [3200, 3200], [10, 20], true, true),
        Some((81, [1, 0, 0x8A, 0x0C, 0x0C, 0x94, 125]))
    );
    // OPOBJ2-6 share the wire payload and select only the protocol opcode.
    let payload = [1, 0, 0x8A, 0x0C, 0x0C, 0x94, 128];
    assert_eq!(
        build_opobj(19, 1, [3200, 3200], [10, 20], false, false),
        Some((crate::proto::client::OPOBJ2, payload))
    );
    assert_eq!(
        build_opobj(20, 1, [3200, 3200], [10, 20], false, false),
        Some((crate::proto::client::OPOBJ3, payload))
    );
    assert_eq!(
        build_opobj(21, 1, [3200, 3200], [10, 20], false, false),
        Some((crate::proto::client::OPOBJ4, payload))
    );
    assert_eq!(
        build_opobj(22, 1, [3200, 3200], [10, 20], false, false),
        Some((crate::proto::client::OPOBJ5, payload))
    );
    assert_eq!(
        build_opobj(1004, 1, [3200, 3200], [10, 20], false, false),
        Some((crate::proto::client::OPOBJ6, payload))
    );
    assert_eq!(
        build_opobj(17, 1, [3200, 3200], [10, 20], false, false),
        None
    );
}

#[test]
fn opnpct_packet_bytes_follow_wire_order() {
    // `p4(parent)` + `p2(npc)` + `p1_alt1(ctrl)` + `p2_alt1(invobject)` +
    // `p2(activeId)` for action 8 -> `OPNPCT` (113). `p4` is big-endian;
    // `p1_alt1` is `v + 128`; `p2_alt1` is lo, hi.
    assert_eq!(
        build_opnpct(
            8,
            0x1234,
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678
            },
            false
        ),
        Some((
            113,
            [0x01, 0x02, 0x03, 0x04, 0x12, 0x34, 128, 0x34, 0x12, 0x56, 0x78]
        ))
    );
    assert_eq!(
        build_opnpct(
            8,
            0x1234,
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678
            },
            true
        ),
        Some((
            113,
            [0x01, 0x02, 0x03, 0x04, 0x12, 0x34, 129, 0x34, 0x12, 0x56, 0x78]
        ))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_opnpct(2008, 0x0001, ActiveTarget::default(), true),
        build_opnpct(8, 0x0001, ActiveTarget::default(), true)
    );
    assert_eq!(build_opnpct(9, 1, ActiveTarget::default(), false), None);
    assert_eq!(build_opnpct(2, 1, ActiveTarget::default(), false), None);
}

#[test]
fn opobjt_packet_bytes_follow_wire_order() {
    // `p2_alt1(obj)` + `p1_alt1(ctrl)` + `p2_alt1(invobject)` +
    // `p2_alt1(z)` + `p2_alt1(x)` + `p4_alt2(parent)` + `p2_alt3(activeId)`
    // for action 17 -> `OPOBJT` (114). `p4_alt2` is `b8, b0, b24, b16`;
    // `p2_alt3` is `lo + 128`, hi. World x 3210 (0x0C8A), z 3220 (0x0C94);
    // no submenu flag, unlike `build_opobj`.
    assert_eq!(
        build_opobjt(
            17,
            0x1234,
            [3200, 3200],
            [10, 20],
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678,
            },
            false
        ),
        Some((
            114,
            [
                0x34, 0x12, 128, 0x34, 0x12, 0x94, 0x0C, 0x8A, 0x0C, 0x03, 0x04, 0x01, 0x02, 0xF8,
                0x56
            ]
        ))
    );
    assert_eq!(
        build_opobjt(
            17,
            0x1234,
            [3200, 3200],
            [10, 20],
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678,
            },
            true
        ),
        Some((
            114,
            [
                0x34, 0x12, 129, 0x34, 0x12, 0x94, 0x0C, 0x8A, 0x0C, 0x03, 0x04, 0x01, 0x02, 0xF8,
                0x56
            ]
        ))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_opobjt(
            2017,
            1,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        ),
        build_opobjt(
            17,
            1,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        )
    );
    assert_eq!(
        build_opobjt(
            18,
            1,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        ),
        None
    );
}

#[test]
fn oploct_packet_bytes_follow_wire_order() {
    // `p1_alt1(ctrl)` + `p2_alt1(x)` + `p2_alt1(invobject)` + `p2_alt3(z)` +
    // `p4_alt1(parent)` + `p4_alt2(id)` + `p2(activeId)` for action 2 ->
    // `OPLOCT` (21). `p4_alt1` is little-endian; the id is the
    // `encode_loc_id` high half masked with `MAX_VALUE`. World x 3210
    // (0x0C8A), z 3220 (0x0C94); id 1234 (0x000004D2).
    let entity = encode_loc_id(1234, 10, 20, 2, 1, false, false);
    assert_eq!(
        build_oploct(
            2,
            entity,
            [3200, 3200],
            [10, 20],
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678,
            },
            false
        ),
        Some((
            21,
            [
                128, 0x8A, 0x0C, 0x34, 0x12, 0x14, 0x0C, 0x04, 0x03, 0x02, 0x01, 0x04, 0xD2, 0x00,
                0x00, 0x56, 0x78
            ]
        ))
    );
    assert_eq!(
        build_oploct(
            2,
            entity,
            [3200, 3200],
            [10, 20],
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678,
            },
            true
        ),
        Some((
            21,
            [
                129, 0x8A, 0x0C, 0x34, 0x12, 0x14, 0x0C, 0x04, 0x03, 0x02, 0x01, 0x04, 0xD2, 0x00,
                0x00, 0x56, 0x78
            ]
        ))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_oploct(
            2002,
            entity,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        ),
        build_oploct(
            2,
            entity,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        )
    );
    assert_eq!(
        build_oploct(
            3,
            entity,
            [3200, 3200],
            [10, 20],
            ActiveTarget::default(),
            false
        ),
        None
    );
}

#[test]
fn apcoordt_packet_bytes_follow_wire_order() {
    // `p2_alt2(x)` + `p4_alt1(parent)` + `p2(invobject)` + `p2_alt2(z)` +
    // `p2_alt2(activeId)` for action 59 -> `APCOORDT` (59). `p2_alt2` is hi,
    // `lo + 128`. World x 3210 (0x0C8A), z 3220 (0x0C94). No ctrl byte.
    assert_eq!(
        build_apcoordt(
            59,
            [3200, 3200],
            [10, 20],
            ActiveTarget {
                parentlayer: 0x01020304,
                invobject: 0x1234,
                id: 0x5678
            }
        ),
        Some((
            59,
            [0x0C, 0x0A, 0x04, 0x03, 0x02, 0x01, 0x12, 0x34, 0x0C, 0x14, 0x56, 0xF8]
        ))
    );
    // 2000 marks a deprioritised option and is stripped before matching.
    assert_eq!(
        build_apcoordt(2059, [3200, 3200], [10, 20], ActiveTarget::default()),
        build_apcoordt(59, [3200, 3200], [10, 20], ActiveTarget::default())
    );
    assert_eq!(
        build_apcoordt(60, [0, 0], [0, 0], ActiveTarget::default()),
        None
    );
}
