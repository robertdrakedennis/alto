use super::*;

/// Append a NUL-terminated string the way `pjstr` encodes it.
fn push_str(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(text.as_bytes());
    out.push(0);
}

#[test]
fn loc_decodes_footprint_models_and_name() {
    // width 3 (14), length 2 (15), one shape (10) with models 100 + 200,
    // name "Bank booth".
    let mut data = vec![14, 3, 15, 2, 1, 1, 10, 2, 0, 100, 0, 200, 2];
    push_str(&mut data, "Bank booth");
    data.push(0);

    let loc = decode_loc(800, &data).unwrap();
    assert_eq!(loc.id, 800);
    assert_eq!(loc.name, "Bank booth");
    assert_eq!(loc.width, 3);
    assert_eq!(loc.length, 2);
    assert_eq!(loc.models, vec![100, 200]);
    // Untouched fields keep their defaults.
    assert_eq!(loc.blockwalk, 2);
    assert!(loc.blockrange);
    assert_eq!(loc.desc.as_deref(), Some("Examine"));
    assert!(loc.recol_s.is_empty());
    assert!(loc.ops.iter().all(|op| op.is_none()));
}

#[test]
fn loc_blockwalk_and_blockrange_opcodes() {
    // 17 -> blockwalk 0.
    let loc = decode_loc(1, &[17, 0]).unwrap();
    assert_eq!(loc.blockwalk, 0);
    assert!(loc.blockrange);

    // 27 -> blockwalk 1.
    let loc = decode_loc(1, &[27, 0]).unwrap();
    assert_eq!(loc.blockwalk, 1);

    // 74 (breaks route finding) forces blockwalk 0 after decoding, even
    // when 27 ran first.
    let loc = decode_loc(1, &[27, 74, 0]).unwrap();
    assert_eq!(loc.blockwalk, 0);

    // 18 clears blockrange.
    let loc = decode_loc(1, &[18, 0]).unwrap();
    assert!(!loc.blockrange);
}

#[test]
fn loc_recol_retex_and_ops() {
    // 40: one recolor 0x1234 -> 0x5678. 41: one retexture 0x0001 -> 0x0002.
    // 30: op[0] "Open". 150: overwrites op[0] with "Bank".
    let mut data = vec![40, 1, 0x12, 0x34, 0x56, 0x78, 41, 1, 0, 1, 0, 2, 30];
    push_str(&mut data, "Open");
    data.push(150);
    push_str(&mut data, "Bank");
    data.push(0);

    let loc = decode_loc(2, &data).unwrap();
    assert_eq!(loc.recol_s, vec![0x1234]);
    assert_eq!(loc.recol_d, vec![0x5678]);
    assert_eq!(loc.retex_s, vec![1]);
    assert_eq!(loc.retex_d, vec![2]);
    assert_eq!(loc.ops[0].as_deref(), Some("Bank"));
    assert!(loc.ops[1..].iter().all(|op| op.is_none()));
}

#[test]
fn loc_operation_cursors_follow_the_opcode_table() {
    // 30: op[0] "Open"; 190 and 194 set the cursors for op[0] and op[4].
    let mut data = vec![30];
    push_str(&mut data, "Open");
    data.extend_from_slice(&[190, 0x01, 0x23, 194, 0x04, 0x56, 0]);

    let loc = decode_loc(3, &data).unwrap();
    assert_eq!(loc.cursor[0], 0x0123);
    assert_eq!(loc.cursor[4], 0x0456);
    assert_eq!(loc.cursor[1], -1);
}

#[test]
fn loc_params_are_retained_for_target_mode_queries() {
    // Opcode 249: one integer param, key 42, value 7.
    let data = [249, 1, 0, 0, 0, 42, 0, 0, 0, 7, 0];
    let loc = decode_loc(4, &data).unwrap();
    assert_eq!(loc.params, vec![(42, ParamValue::Int(7))]);
}

#[test]
fn loc_shade_and_palette_retention() {
    // 29: ambient -10. 39: contrast byte 3 -> 15 (stored as five times the byte).
    // 42: palette [7, -8]. 163: tint (1, 2, 3, weight 4).
    let data = [29, 246, 39, 3, 42, 2, 7, 248, 163, 1, 2, 3, 4, 0];
    let loc = decode_loc(9, &data).unwrap();
    assert_eq!(loc.ambient, -10);
    assert_eq!(loc.contrast, 15);
    assert_eq!(loc.recol_d_palette, vec![7_i8, -8]);
    assert_eq!(
        (
            loc.tint_hue,
            loc.tint_saturation,
            loc.tint_luminence,
            loc.tint_weight
        ),
        (1, 2, 3, 4)
    );
    // Untouched locs keep the defaults.
    let plain = decode_loc(10, &[0]).unwrap();
    assert_eq!(plain.ambient, 0);
    assert_eq!(plain.contrast, 0);
    assert!(plain.recol_d_palette.is_empty());
    assert_eq!(
        (
            plain.tint_hue,
            plain.tint_saturation,
            plain.tint_luminence,
            plain.tint_weight
        ),
        (0, 0, 0, 0)
    );
}

#[test]
fn loc_wide_model_id_drops_null_entries() {
    // Opcode 1 with a 4-byte wide model id (high bit set: g4s & MAX) and a
    // 32767 null entry (dropped from the flattened list).
    let data = vec![1, 1, 22, 2, 0x80, 0, 0x01, 0x00, 0x7F, 0xFF, 0];
    let loc = decode_loc(3, &data).unwrap();
    assert_eq!(loc.models, vec![0x100]);
}

#[test]
fn loc_explicit_noop_opcodes_decode() {
    // 168/169/188/198/199 carry no bytes (accepted and ignored); 69/99/100
    // carry payload that is consumed and dropped.
    let data = [168, 169, 188, 198, 199, 69, 7, 99, 1, 0, 2, 100, 3, 0, 4, 0];
    let loc = decode_loc(4, &data).unwrap();
    assert_eq!(loc.width, 1);
    assert_eq!(loc.blockwalk, 2);
}

#[test]
fn loc_unknown_opcode_errors_with_id_and_opcode() {
    // Opcode 3 is in no table.
    let error = decode_loc(1234, &[3, 0]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("1234"), "missing id: {message}");
    assert!(message.contains('3'), "missing opcode: {message}");

    // Missing terminator is an error, not a panic.
    assert!(decode_loc(5, &[14, 3]).is_err());
    assert!(decode_loc(6, &[]).is_err());
}

#[test]
fn obj_decodes_mesh_name_and_ops() {
    // mesh 512 (1), name "Coins" (2), op[0] "Take" (30), iop[4] "Drop".
    let mut data = vec![1, 2, 0, 2];
    push_str(&mut data, "Coins");
    data.push(30);
    push_str(&mut data, "Take");
    data.push(39);
    push_str(&mut data, "Drop");
    data.extend_from_slice(&[40, 1, 0, 10, 0, 20]);
    data.push(0);

    let obj = decode_obj(10, &data).unwrap();
    assert_eq!(obj.id, 10);
    assert_eq!(obj.name, "Coins");
    assert_eq!(obj.models, vec![512]);
    assert_eq!(obj.ops[0].as_deref(), Some("Take"));
    assert_eq!(obj.iops[4].as_deref(), Some("Drop"));
    assert_eq!(obj.recol_s, vec![10]);
    assert_eq!(obj.recol_d, vec![20]);
}

#[test]
fn obj_explicit_noop_and_unknown_opcodes() {
    // 156 is silently ignored; 15/16/65/157/165/167/168 carry no bytes.
    let obj = decode_obj(11, &[156, 15, 16, 65, 157, 165, 167, 168, 0]).unwrap();
    assert_eq!(obj.name, "null");

    let error = decode_obj(77, &[3, 0]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("77"), "missing id: {message}");
}

#[test]
fn npc_decodes_models_name_size_and_bas() {
    // models [50, 51] (1), name "Goblin" (2), size 2 (12), bas 200 (127).
    let mut data = vec![1, 2, 0, 50, 0, 51, 2];
    push_str(&mut data, "Goblin");
    data.extend_from_slice(&[12, 2, 127, 0, 200, 0]);

    let npc = decode_npc(20, &data).unwrap();
    assert_eq!(npc.id, 20);
    assert_eq!(npc.name, "Goblin");
    assert_eq!(npc.models, vec![50, 51]);
    assert_eq!(npc.size, 2);
    assert_eq!(npc.bas, 200);
}

#[test]
fn npc_menu_cursors_and_params_follow_the_opcode_table() {
    // 30: op[0] "Talk"; 170/175 set cursor slots 0/5; 249 stores an
    // integer target parameter. 137/159 cover attack cursor and the
    // explicit no-reprioritisation flag.
    let mut data = vec![30];
    push_str(&mut data, "Talk");
    data.extend_from_slice(&[
        170, 0x01, 0x23, 175, 0xFF, 0xFF, 137, 0x02, 0x34, 159, 249, 1, 0, 0, 0, 42, 0, 0, 0, 9, 0,
    ]);

    let npc = decode_npc(21, &data).unwrap();
    assert_eq!(npc.cursor[0], 0x0123);
    assert_eq!(npc.cursor[5], -1);
    assert_eq!(npc.cursorattack, 0x0234);
    assert_eq!(npc.reprioritise_attack_op, 0);
    assert_eq!(npc.params, vec![(42, ParamValue::Int(9))]);
    assert_eq!(decode_npc(22, &[0]).unwrap().reprioritise_attack_op, 1);
}

#[test]
fn npc_draw_options_follow_the_opcode_table() {
    // 111 no shadow; 113 shadow colours, 114 their transparencies; 181 a
    // shadow material and its transparency; 119 walk flags; 123 the overhead
    // height; 163/165 pick size and its shift; 169 no colour shift; 179 a
    // clickbox as six signed smarts; 180 the fade-in duration.
    let data = [
        111,
        113,
        0x21,
        0x43,
        0xeb,
        0x6f,
        114,
        0xa0,
        0xf0,
        181,
        0x10,
        0xe1,
        0x64,
        119,
        3,
        123,
        0x01,
        0x2c,
        163,
        2,
        165,
        1,
        169,
        179,
        64,
        64 + 3,
        64,
        64 + 10,
        64 + 20,
        64 + 5,
        180,
        20,
        0,
    ];
    let npc = decode_npc(23, &data).unwrap();
    assert!(!npc.spotshadow);
    assert_eq!(npc.spotshadow_colours, [0x2143, 0xeb6f]);
    assert_eq!(npc.spotshadow_trans, [160, 240]);
    assert_eq!(
        (npc.spotshadow_texture, npc.spotshadow_texture_alpha),
        (4321, 100)
    );
    assert_eq!(npc.walkflags, 3);
    assert_eq!(npc.overlayheight, 300);
    assert_eq!((npc.picksize, npc.picksizeshift), (2, 1));
    assert!(!npc.antimacro);
    assert_eq!(npc.clickbox, Some([0, 3, 0, 10, 20, 5]));
    assert_eq!(npc.fade_in, 20);
    // A type that says nothing casts a shadow, shifts its colours, is
    // opaque from the start and has no clickbox.
    let plain = decode_npc(24, &[0]).unwrap();
    assert!(plain.spotshadow && plain.antimacro);
    assert_eq!(
        (plain.fade_in, plain.overlayheight, plain.walkflags),
        (0, -1, 0)
    );
    assert_eq!(plain.spotshadow_trans, [160, 240]);
    assert_eq!(plain.clickbox, None);
}

#[test]
fn npc_explicit_noop_and_unknown_opcodes() {
    // 162/178 are silently ignored; 93/99/107/109/111/141/143/158/159/169/182
    // carry no bytes.
    let npc = decode_npc(
        21,
        &[
            162, 178, 93, 99, 107, 109, 111, 141, 143, 158, 159, 169, 182, 0,
        ],
    )
    .unwrap();
    assert_eq!(npc.size, 1);
    assert_eq!(npc.bas, -1);

    let error = decode_npc(88, &[3, 0]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("88"), "missing id: {message}");
}

#[test]
fn seq_decodes_frame_ids_and_lengths() {
    // Opcode 1: count 2, lengths [5, 7], lo [10, 20], hi [0, 1] ->
    // ids [10, 20 | 1 << 16].
    let data = [1, 0, 2, 0, 5, 0, 7, 0, 10, 0, 20, 0, 0, 0, 1, 0];
    let seq = decode_seq(30, &data).unwrap();
    assert_eq!(seq.id, 30);
    assert_eq!(seq.frame_lengths, vec![5, 7]);
    assert_eq!(seq.frame_ids, vec![10, 20 | (1 << 16)]);
}

#[test]
fn seq_noop_and_unknown_opcodes() {
    // 14/15 carry no bytes; 16/18 are silently ignored.
    let seq = decode_seq(31, &[14, 15, 16, 18, 0]).unwrap();
    assert!(seq.frame_ids.is_empty());

    let error = decode_seq(99, &[3, 0]).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("99"), "missing id: {message}");
}

/// Real config packs.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_pack_obj_npc_seq_counts() {
    let pack = crate::test_support::require_pack("client.obj.config.js5");
    let objs = ObjStore::load(&pack).unwrap();
    let npcs = NpcStore::load(&pack).unwrap();
    let seqs = SeqStore::load(&pack).unwrap();
    println!(
        "counts: objs={} npcs={} seqs={}",
        objs.len(),
        npcs.len(),
        seqs.len()
    );
    assert!(!objs.is_empty());
    assert!(!npcs.is_empty());
    assert!(!seqs.is_empty());

    // Named spot-checks every consumer relies on existing.
    assert!(
        objs.iter().any(|(_, obj)| obj.name != "null"),
        "expected some named objs"
    );
    assert!(
        npcs.iter().any(|(_, npc)| npc.name != "null"),
        "expected some named npcs"
    );
    assert!(
        npcs.iter().any(|(_, npc)| !npc.models.is_empty()),
        "expected some modelled npcs"
    );
    assert!(
        seqs.iter().any(|(_, seq)| !seq.frame_ids.is_empty()),
        "expected some seqs with frames"
    );
}

/// The record reader's own rules on top of `rs910_core::reader`: text is
/// Windows-1252 with unassigned bytes decoded as `?` and a required
/// terminator; the nullable smart and the id smart encode "none"
/// as `-1`, and `peek` does not advance.
#[test]
fn reader_strings_are_strict_and_null_smarts_decode_as_none() {
    use crate::opcode_table::{ConfigReader, Source};
    let data = [b'A', 0x80, 0, 0x00, 0x80, 0x00, 0x7F, 0xFF, 0x80, 0, 0, 7];
    let mut r = ConfigReader::new(&data, "config");
    assert_eq!(r.text().unwrap(), "A\u{20AC}");
    assert_eq!(r.peek().unwrap(), 0);
    assert_eq!(r.smart_nullable().unwrap(), -1, "one byte, minus one");
    assert_eq!(r.smart_nullable().unwrap(), -1, "two bytes, minus 32769");
    assert_eq!(r.smart_id().unwrap(), -1, "32767 is none");
    assert_eq!(r.smart_id().unwrap(), 7, "four bytes, top bit cleared");
    assert!(r.byte().is_err());

    assert_eq!(
        ConfigReader::new(&[b'A', 0x81, 0], "config")
            .text()
            .unwrap(),
        "A?"
    );
    assert!(ConfigReader::new(b"AB", "config").text().is_err());
}
