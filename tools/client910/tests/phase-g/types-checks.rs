#[allow(unused_imports)]
use crate::{entities910, protocol910};
use protocol910::{config_types::*, Error};
#[test]
fn missing_records_and_invalid_existing_records_are_distinct() {
    let raw = std::collections::BTreeMap::new();
    let t = resolve_items(&raw, [77], false).unwrap();
    assert_eq!(t[&77], Item::empty(77));
    for bytes in [
        &[][..],
        &[12, 0, 0][..],
        &[23, 128][..],
        &[40, 1, 0, 1, 0][..],
    ] {
        assert!(Item::decode(77, bytes).is_err());
    }
    assert!(matches!(
        Item::decode(7, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "item",
            id: 7,
            opcode: 250
        })
    ));
    assert!(matches!(
        Npc::decode(7, &[250, 0]),
        Err(Error::UnsupportedConfig {
            kind: "NPC",
            id: 7,
            opcode: 250
        })
    ));
}
#[test]
fn derivation_cycles_and_zero_divisors_never_return_partial_types() {
    let mut a = Item::empty(1);
    a.derived[0] = [2, 2];
    let mut b = Item::empty(2);
    b.derived[0] = [1, 1];
    let raw: std::collections::BTreeMap<i32, Item> = [(1, a), (2, b)].into_iter().collect();
    let before = raw.clone();
    assert!(resolve_items(&raw, [1, 2], true).is_err());
    assert_eq!(raw, before);
    let mut a = Item::empty(1);
    a.derived[3] = [2, 2];
    let raw = [(1, a), (2, Item::empty(2))].into_iter().collect();
    assert!(matches!(
        resolve_items(&raw, [1], true),
        Err(Error::Invalid("zero shard count"))
    ));
}
#[test]
fn offsets_require_a_valid_model_array() {
    assert!(Npc::decode(1, &[121, 0, 0]).is_err());
    assert!(Npc::decode(1, &[1, 0, 121, 1, 0, 1, 2, 3, 0]).is_err());
    let a = Npc::decode(1, &[1, 1, 0, 9, 121, 1, 0, 128, 255, 127, 0]).unwrap();
    assert_eq!(a.modeloffset, Some(vec![Some([-128, -1, 127])]));
}
#[test]
fn unsafe_dependency_types_stay_explicitly_blocked_at_packet_boundary() {
    let mut n = Npc::empty(1);
    n.sounds[1] = 9;
    assert!(!n.packet_type().has_sound_or_multinpc);
    n.sounds[0] = 9;
    assert!(n.packet_type().has_sound_or_multinpc);
    n.sounds.fill(-1);
    n.multinpc = Some(vec![-1, -1]);
    assert!(n.packet_type().has_sound_or_multinpc);
}
