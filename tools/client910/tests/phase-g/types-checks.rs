use protocol910::{config_types::*, Error};
#[allow(unused_imports)]
use rs910_game::{entities910, protocol910};
#[test]
fn unknown_npc_opcodes_name_the_record() {
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
