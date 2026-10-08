//! The members gate on real cache types.
use super::*;
use crate::cache::Pack;

fn pack() -> Pack {
    Pack::open(rs910_core::test_support::pack_root())
}

/// One type file straight from its config group (`id >>> bits`, `id & mask`).
fn raw(pack: &Pack, archive: &str, bits: u32, id: u32) -> Vec<u8> {
    let files = pack.read_group(archive, id >> bits).unwrap();
    files
        .into_iter()
        .find(|(file, _)| *file == id & ((1 << bits) - 1))
        .map(|(_, bytes)| bytes)
        .unwrap()
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_npc_members_only_ops_are_null_without_allow_members() {
    // NPC 44 "Banker": ops 0/2/3 come from the members opcode family.
    let pack = pack();
    let banker = decode_npc(44, &raw(&pack, NPC_ARCHIVE, NPC_GROUP_BITS, 44)).unwrap();
    assert_eq!(banker.name, "Banker");
    assert_eq!(banker.members_op_slots, 0b1101);
    assert_eq!(
        banker.ops_for(true),
        [Some("Bank"), None, Some("Talk-to"), Some("Collect"), None]
    );
    // A free world stores nothing for those slots.
    assert_eq!(banker.ops_for(false), [None; 5]);
    let store = NpcStore::from_map(BTreeMap::from([(44, banker)]));
    assert!(
        store.allow_members.get(),
        "lists start with members allowed"
    );
    assert!(store.allow_members.set(false), "a change is reported");
    assert!(!store.allow_members.set(false), "an unchanged value is not");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_loc_members_gate_ops_quests_and_active() {
    let pack = pack();
    // A members loc loses every op on a free world.
    let chest = decode_loc(172, &raw(&pack, LOC_ARCHIVE, LOC_GROUP_BITS, 172)).unwrap();
    assert_eq!(chest.name, "Crystal chest");
    assert!(chest.members);
    assert_eq!(chest.ops_for(true)[0], Some("Open"));
    assert_eq!(chest.ops_for(false), [None; 5]);
    // Loc 215 "Wall" only has members-family ops, so it is inactive on a
    // free world.
    let wall = decode_loc(215, &raw(&pack, LOC_ARCHIVE, LOC_GROUP_BITS, 215)).unwrap();
    assert!(!wall.members);
    assert_eq!(wall.ops_for(false), [None; 5]);
    assert_eq!((wall.active_for(true), wall.active_for(false)), (1, 0));
    // Loc 470 "Ivy": an explicit active value keeps it active on either world.
    let ivy = decode_loc(470, &raw(&pack, LOC_ARCHIVE, LOC_GROUP_BITS, 470)).unwrap();
    assert_eq!((ivy.active_for(true), ivy.active_for(false)), (1, 1));
    assert_eq!(ivy.ops_for(false)[0], None);
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn real_obj_members_gate_matches_postdecode() {
    let pack = pack();
    let params: BTreeMap<i32, native910::config::ParamConfig> = pack
        .read_group("config", 11)
        .unwrap()
        .iter()
        .filter_map(|(id, bytes)| Some((*id as i32, native910::config::decode_param(bytes).ok()?)))
        .collect();
    let autodisable = |key: i32| params.get(&key).is_none_or(|p| p.autodisable);
    let whip = decode_obj(4151, &raw(&pack, OBJ_ARCHIVE, OBJ_GROUP_BITS, 4151)).unwrap();
    assert_eq!(whip.name, "Abyssal whip");
    assert!(whip.members);
    assert_eq!(whip.iops[1].as_deref(), Some("Wield"));
    assert!(matches!(
        whip.members_gated(true, &autodisable),
        std::borrow::Cow::Borrowed(_)
    ));
    let gated = whip.members_gated(false, &autodisable);
    assert_eq!(gated.ops, obj_default_ops());
    assert_eq!(gated.iops, obj_default_iops());
    assert!(!gated.inventory.tradeable && !gated.inventory.stockmarket);
    assert!(gated.inventory.quests.is_empty());
    assert!(
        gated.params.len() < whip.params.len(),
        "autodisable params pruned"
    );
    assert!(gated.params.iter().all(|(key, _)| !autodisable(*key)));
    // Free objects are untouched.
    let partyhat = decode_obj(1038, &raw(&pack, OBJ_ARCHIVE, OBJ_GROUP_BITS, 1038)).unwrap();
    assert!(!partyhat.members);
    assert!(matches!(
        partyhat.members_gated(false, &autodisable),
        std::borrow::Cow::Borrowed(_)
    ));
}
