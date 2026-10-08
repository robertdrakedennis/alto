use super::*;
#[test]
fn menu_and_packet_corpus_matches_recording() -> anyhow::Result<()> {
    let mut out = vec![];
    fn int(b: &mut Vec<u8>, v: i32) {
        b.extend(v.to_be_bytes());
    }
    fn string(b: &mut Vec<u8>, v: &str) {
        assert!(v.is_ascii());
        b.extend((v.len() as u16).to_be_bytes());
        b.extend(v.bytes());
    }
    int(&mut out, 20);
    for test in 0..20 {
        let names = [
            Some("Trade"),
            Some("Attack"),
            Some("Follow"),
            None,
            None,
            None,
            None,
            None,
        ];
        let mut input = PlayerOptionInput {
            option_count: 1,
            same_plane: true,
            local: false,
            index: 2,
            name: "Target",
            transformed: false,
            combat_level: 40,
            max_combat_level: 40,
            skill_level: 0,
            local_combat_level: 50,
            local_wilderness_level: -1,
            wilderness_level: -1,
            local_team: 0,
            team: 0,
            suppress_partner_highlight: false,
            options: &names,
            deprioritised: [false; 8],
            cursors: [-1; 8],
            target_mode: false,
            target_mask: 0,
            target_verb: "Cast",
            target_name: "Spell",
            target_cursor: 12,
            attack_priority: AttackPriority::HigherLevelRight,
            default_cursor: -1,
        };
        match test {
            1 => input.skill_level = -1,
            2 => input.skill_level = 1000,
            3..=11 => {
                input.combat_level = 50 - [-10, -7, -4, -1, 0, 1, 4, 7, 10][test - 3];
                input.max_combat_level = input.combat_level;
            }
            12 => {
                input.local_wilderness_level = 1;
                input.wilderness_level = 1;
                input.combat_level = 70;
                input.max_combat_level = 70;
            }
            13 | 14 => {
                input.attack_priority = AttackPriority::AlwaysRight;
                input.local_team = 1;
                input.team = if test == 13 { 2 } else { 1 };
            }
            15 => {
                input.deprioritised[2] = true;
                input.cursors[2] = 17;
            }
            16 => {
                input.local = true;
                input.index = 1;
                input.target_mode = true;
                input.target_mask = 16;
            }
            17 => {
                input.target_mode = true;
                input.target_mask = 8;
            }
            18 => input.option_count = 407,
            19 => input.max_combat_level = 50,
            _ => {}
        }
        let entries = build_player_options(&input);
        int(&mut out, entries.len() as i32);
        for e in entries {
            string(&mut out, &e.op);
            string(&mut out, e.target.as_deref().unwrap_or(""));
            int(&mut out, e.action);
            int(&mut out, e.cursor);
            out.extend(e.entity_id.to_be_bytes());
            out.extend(e.sub_id.to_be_bytes());
            string(&mut out, e.detail.as_deref().unwrap_or(""));
        }
    }
    let expected = rs910_core::test_support::frozen::bytes("player-options/menu.bin");
    anyhow::ensure!(
        out == expected,
        "menu mismatch at byte {:?}; lengths {}/{}",
        out.iter().zip(&expected).position(|(a, b)| a != b),
        out.len(),
        expected.len()
    );
    let mut packets = vec![];
    for action in 44..=53 {
        for ctrl in [false, true] {
            let (op, payload) = build_opplayer_packet(action, 0x1234, ctrl).unwrap();
            packets.push(op);
            packets.extend(payload);
        }
    }
    assert_eq!(
        packets,
        rs910_core::test_support::frozen::bytes("player-options/packets.bin")
    );
    Ok(())
}
