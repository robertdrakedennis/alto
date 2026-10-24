use super::*;

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
