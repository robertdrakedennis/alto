use protocol910::config_types::*;
#[allow(unused_imports)]
use rs910_game::{entities910, protocol910};
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
