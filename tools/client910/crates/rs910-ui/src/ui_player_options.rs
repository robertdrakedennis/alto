//! The player right-click menu builder.
//! This is deliberately data-only: the scene owner supplies the already
//! resolved player attributes and the caller inserts the returned entries.
//!
//! On-target packet builders: `build_opplayert` owns actions 15/16 ->
//! `OPPLAYERT` and `build_if_buttont` owns action 58 -> `IF_BUTTONT`.
//! Target selection state (the active component and its parent layer) stays
//! partial: no picker or dispatch is built here, only the wire bytes. The
//! live `IF_BUTTONT` dispatch in `ui_interaction.rs` is untouched.

use crate::ui_minimenu::Entry;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttackPriority {
    Default,
    Hidden,
    AlwaysRight,
    HigherLevelRight,
}

pub struct PlayerOptionInput<'a> {
    pub option_count: usize,
    pub same_plane: bool,
    pub local: bool,
    pub index: i32,
    /// The player's display name with its extras, or for a
    /// `transmogfakenpc` player the complete NPC label.
    pub name: &'a str,
    /// The player model is a `transmogfakenpc` NPC.
    pub transformed: bool,
    pub combat_level: i32,
    pub max_combat_level: i32,
    pub skill_level: i32,
    pub local_combat_level: i32,
    pub local_wilderness_level: i32,
    pub wilderness_level: i32,
    pub local_team: i32,
    pub team: i32,
    pub suppress_partner_highlight: bool,
    pub options: &'a [Option<&'a str>; 8],
    pub deprioritised: [bool; 8],
    pub cursors: [i32; 8],
    pub target_mode: bool,
    pub target_mask: i32,
    pub target_verb: &'a str,
    pub target_name: &'a str,
    pub target_cursor: i32,
    pub attack_priority: AttackPriority,
    pub default_cursor: i32,
}

pub fn build_player_options(input: &PlayerOptionInput<'_>) -> Vec<Entry> {
    let mut out = Vec::new();
    if input.option_count >= 407 {
        return out;
    }
    if !input.local && !input.same_plane {
        // Another level shows one disabled grey row, unless the player is
        // drawn as a fake NPC.
        if !input.transformed {
            out.push(Entry {
                op: format!("<col=cccccc>{}", player_name_text(input)),
                target: Some(String::new()),
                cursor: -1,
                action: -1,
                obj_id: 0,
                entity_id: 0,
                tile_x: 0,
                tile_z: 0,
                enabled: false,
                has_arrow: true,
                sub_id: input.index as i64,
                force_submenu: false,
                detail: None,
            });
        }
        return out;
    }
    if !input.same_plane {
        // The local-player target row needs no level check, but the local
        // player never reaches the other-level scene pass.
        return out;
    }
    if input.local {
        if input.target_mode && input.target_mask & 0x10 != 0 {
            let mut e = entry(
                input.target_verb,
                &format!(
                    "{} -> <col=ffffff>{}",
                    input.target_name,
                    rs910_core::texts::Msg::SelfLabel.get()
                ),
                input.target_cursor,
                16,
                0,
            );
            e.sub_id = input.index as i64;
            out.push(e);
        }
        return out;
    }
    // Fake-NPC players are yellow, others white.
    let suffix = if input.transformed {
        format!("<col=ffff00>{}", player_name_text(input))
    } else {
        player_label(input)
    };
    if input.target_mode && input.target_mask & 8 != 0 {
        out.push(entry(
            input.target_verb,
            &format!("{} -> {}", input.target_name, player_label(input)),
            input.target_cursor,
            15,
            input.index,
        ));
    }
    for i in (0..8).rev() {
        let Some(op) = input.options[i] else { continue };
        if op.eq_ignore_ascii_case(rs910_core::texts::Msg::Attack.get())
            && matches!(input.attack_priority, AttackPriority::Hidden)
        {
            continue;
        }
        let mut action = 44 + i as i32;
        if !op.eq_ignore_ascii_case(rs910_core::texts::Msg::Attack.get()) && input.deprioritised[i]
        {
            action += 2000;
        }
        if op.eq_ignore_ascii_case(rs910_core::texts::Msg::Attack.get()) {
            let higher = matches!(input.attack_priority, AttackPriority::AlwaysRight)
                || matches!(input.attack_priority, AttackPriority::HigherLevelRight)
                    && input.combat_level > input.local_combat_level;
            let right = if input.local_team != 0 && input.team != 0 {
                input.local_team == input.team
            } else {
                higher || input.suppress_partner_highlight
            };
            if right {
                action += 2000;
            }
        }
        let cursor = if input.cursors[i] == -1 {
            input.default_cursor
        } else {
            input.cursors[i]
        };
        out.push(entry(op, &suffix, cursor, action, input.index));
    }
    out
}

/// The white-tagged player name: the target-mode row and the `Walk here`
/// detail text.
pub fn player_label(input: &PlayerOptionInput<'_>) -> String {
    format!("<col=ffffff>{}", player_name_text(input))
}

/// The player's menu name text: the name with its skill or combat level
/// suffix, or the complete NPC label for a `transmogfakenpc` player.
pub fn player_name_text(input: &PlayerOptionInput<'_>) -> String {
    if input.transformed {
        return input.name.to_owned();
    }
    let prefix = input.name.to_owned();
    if input.skill_level == -1 {
        return prefix;
    }
    if input.skill_level != 0 {
        return format!(
            "{} ({}{})",
            prefix,
            rs910_core::texts::Msg::Skill.get(),
            input.skill_level
        );
    }
    let white = input.local_wilderness_level != -1
        && input.wilderness_level != -1
        && input
            .combat_level
            .wrapping_sub(input.local_combat_level)
            .wrapping_abs()
            > input.local_wilderness_level.min(input.wilderness_level);
    let colour = if white {
        "<col=ffffff>"
    } else {
        combat_colour(input.combat_level, input.local_combat_level)
    };
    if input.max_combat_level > input.combat_level {
        format!(
            "{}{} ({}{}+{})",
            prefix,
            colour,
            rs910_core::texts::Msg::Level.get(),
            input.combat_level,
            input.max_combat_level.wrapping_sub(input.combat_level)
        )
    } else {
        format!(
            "{}{} ({}{})",
            prefix,
            colour,
            rs910_core::texts::Msg::Level.get(),
            input.combat_level
        )
    }
}

fn entry(op: &str, target: &str, cursor: i32, action: i32, index: i32) -> Entry {
    Entry {
        op: op.to_owned(),
        target: Some(target.to_owned()),
        cursor,
        action,
        obj_id: -1,
        entity_id: index as i64,
        tile_x: 0,
        tile_z: 0,
        enabled: true,
        has_arrow: false,
        sub_id: index as i64,
        force_submenu: false,
        detail: None,
    }
}

/// Menu actions 44-53 -> `OPPLAYER1..10`: the player index as a big-endian
/// u16, then the ctrl flag as a byte `v + 128`. Opcodes (all size 3):
/// 44->117, 45->62, 46->76, 47->102, 48->49, 49->4, 50->54, 51->48, 52->6,
/// 53->91. `>= 2000` strips the deprioritised flag. The cross-hair mode and
/// minimap flag the original client sets on the target are scene
/// side-effects that stay with the caller; only the wire bytes are built
/// here.
pub fn build_opplayer_packet(action: i32, player_index: u16, ctrl: bool) -> Option<(u8, [u8; 3])> {
    let base = action - if action >= 2000 { 2000 } else { 0 };
    if !(44..=53).contains(&base) {
        return None;
    }
    let opcodes = [117, 62, 76, 102, 49, 4, 54, 48, 6, 91];
    Some((
        opcodes[(base - 44) as usize],
        [
            (player_index >> 8) as u8,
            player_index as u8,
            if ctrl { 129 } else { 128 },
        ],
    ))
}

/// Menu actions 15/16 -> `OPPLAYERT` (opcode 41, size 11), one wire for the
/// self variant (the index is the local player's) and the other-player
/// variant: `activeId` big-endian u16, the ctrl flag as a byte `v + 128`,
/// `activeInvobject` little-endian u16, `playerIndex` as `lo + 128, hi`, and
/// `activeParentlayer` as the bytes `b8, b0, b24, b16`. The caller supplies
/// the index to send, so one builder covers both variants. `>= 2000` strips
/// the deprioritised flag.
pub fn build_opplayert(
    action: i32,
    player_index: u16,
    active_id: i32,
    active_invobject: i32,
    active_parentlayer: i32,
    ctrl: bool,
) -> Option<(u8, [u8; 11])> {
    let base = action - if action >= 2000 { 2000 } else { 0 };
    if base != 15 && base != 16 {
        return None;
    }
    Some((
        crate::proto::client::OPPLAYERT,
        [
            (active_id >> 8) as u8,
            active_id as u8,
            u8::from(ctrl).wrapping_add(128),
            active_invobject as u8,
            (active_invobject >> 8) as u8,
            (player_index as u8).wrapping_add(128),
            (player_index >> 8) as u8,
            (active_parentlayer >> 8) as u8,
            active_parentlayer as u8,
            (active_parentlayer >> 24) as u8,
            (active_parentlayer >> 16) as u8,
        ],
    ))
}

/// The target and active components of an item-on-component use.
pub struct TargetedButtonUse {
    pub target_parentlayer: i32,
    pub target_invobject: i32,
    pub target_id: i32,
    pub active_invobject: i32,
    pub active_id: i32,
    pub active_parentlayer: i32,
}

/// Menu action 58 -> `IF_BUTTONT` (opcode 58, size 16): `targetParentlayer`
/// little-endian i32, `targetInvobject` big-endian u16, `activeInvobject` as
/// `hi, lo + 128`, `targetId` as `hi, lo + 128`, `activeId` as
/// `hi, lo + 128`, then `activeParentlayer` little-endian i32. No ctrl byte
/// is sent here; `>= 2000` still strips the deprioritised flag.
/// Byte-identical to the live `Action::Target` dispatch in
/// `ui_interaction.rs`, which stays the dispatch owner.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "wire-byte builder exercised by tests only")
)]
pub fn build_if_buttont(action: i32, used: &TargetedButtonUse) -> Option<(u8, [u8; 16])> {
    let base = action - if action >= 2000 { 2000 } else { 0 };
    if base != 58 {
        return None;
    }
    let TargetedButtonUse {
        target_parentlayer,
        target_invobject,
        target_id,
        active_invobject,
        active_id,
        active_parentlayer,
    } = *used;
    Some((
        crate::proto::client::IF_BUTTONT,
        [
            target_parentlayer as u8,
            (target_parentlayer >> 8) as u8,
            (target_parentlayer >> 16) as u8,
            (target_parentlayer >> 24) as u8,
            (target_invobject >> 8) as u8,
            target_invobject as u8,
            (active_invobject >> 8) as u8,
            (active_invobject as u8).wrapping_add(128),
            (target_id >> 8) as u8,
            (target_id as u8).wrapping_add(128),
            (active_id >> 8) as u8,
            (active_id as u8).wrapping_add(128),
            active_parentlayer as u8,
            (active_parentlayer >> 8) as u8,
            (active_parentlayer >> 16) as u8,
            (active_parentlayer >> 24) as u8,
        ],
    ))
}

/// The colour tag for a target's combat level relative to the local player's.
pub fn combat_colour(target: i32, local: i32) -> &'static str {
    match local.wrapping_sub(target) {
        d if d < -9 => "<col=ff0000>",
        d if d < -6 => "<col=ff3000>",
        d if d < -3 => "<col=ff7000>",
        d if d < 0 => "<col=ffb000>",
        d if d > 9 => "<col=ff00>",
        d if d > 6 => "<col=40ff00>",
        d if d > 3 => "<col=80ff00>",
        d if d > 0 => "<col=c0ff00>",
        _ => "<col=ffff00>",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn input<'a>(names: &'a [Option<&'a str>; 8]) -> PlayerOptionInput<'a> {
        PlayerOptionInput {
            option_count: 1,
            same_plane: true,
            local: false,
            index: 5,
            name: "Bob",
            transformed: false,
            combat_level: 40,
            max_combat_level: 40,
            skill_level: 0,
            local_combat_level: 40,
            local_wilderness_level: -1,
            wilderness_level: -1,
            local_team: 0,
            team: 0,
            suppress_partner_highlight: false,
            options: names,
            deprioritised: [false; 8],
            cursors: [-1; 8],
            target_mode: true,
            target_mask: 8,
            target_verb: "Cast",
            target_name: "Wind<col=ffffff>",
            target_cursor: 3,
            attack_priority: AttackPriority::HigherLevelRight,
            default_cursor: -1,
        }
    }

    /// Another level gives one disabled grey row with action -1 (no target or operation rows).
    #[test]
    fn other_level_player_is_one_disabled_row() {
        let names = [Some("Follow"), None, None, None, None, None, None, None];
        let mut i = input(&names);
        i.same_plane = false;
        let rows = build_player_options(&i);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.op, "<col=cccccc>Bob<col=ffff00> (level: 40)");
        assert_eq!(row.target.as_deref(), Some(""));
        assert_eq!(
            (row.action, row.obj_id, row.entity_id, row.sub_id),
            (-1, 0, 0, 5)
        );
        assert!(!row.enabled && row.has_arrow);
        i.transformed = true;
        assert!(build_player_options(&i).is_empty());
    }

    /// A `transmogfakenpc` player keeps player actions but uses
    /// the yellow NPC label; the target row keeps the white tag.
    #[test]
    fn transformed_player_uses_yellow_npc_label() {
        let names = [Some("Follow"), None, None, None, None, None, None, None];
        let mut i = input(&names);
        i.transformed = true;
        i.name = "Guard<col=ff00> (level: 21)";
        let rows = build_player_options(&i);
        assert_eq!(rows[0].action, 15);
        assert_eq!(
            rows[0].target.as_deref(),
            Some("Wind<col=ffffff> -> <col=ffffff>Guard<col=ff00> (level: 21)")
        );
        assert_eq!(rows[1].action, 44);
        assert_eq!(
            rows[1].target.as_deref(),
            Some("<col=ffff00>Guard<col=ff00> (level: 21)")
        );
    }

    #[test]
    fn opplayer_packets_match_reference_layout() {
        // Actions 44-53 map to `OPPLAYER1..10`: the index as hi, lo, then the
        // ctrl byte `v + 128` (ctrl false/true -> 128/129), for opcodes
        // (all size 3): 44->117, 45->62, 46->76, 47->102, 48->49, 49->4,
        // 50->54, 51->48, 52->6, 53->91.
        let table: [(i32, u8); 10] = [
            (44, 117),
            (45, 62),
            (46, 76),
            (47, 102),
            (48, 49),
            (49, 4),
            (50, 54),
            (51, 48),
            (52, 6),
            (53, 91),
        ];
        for (action, opcode) in table {
            for ctrl in [false, true] {
                let want_ctrl = if ctrl { 129 } else { 128 };
                assert_eq!(
                    build_opplayer_packet(action, 0x1234, ctrl),
                    Some((opcode, [0x12, 0x34, want_ctrl])),
                    "action {action} ctrl {ctrl}",
                );
                // 2000-offset (deprioritised) variants send the same opcode
                // (2000 is stripped before matching).
                assert_eq!(
                    build_opplayer_packet(action + 2000, 0x1234, ctrl),
                    Some((opcode, [0x12, 0x34, want_ctrl])),
                    "action {} ctrl {ctrl}",
                    action + 2000,
                );
            }
            // Registry framing: every OPPLAYER opcode is fixed size 3.
            assert_eq!(
                crate::proto::client::size(opcode),
                Some(3),
                "opcode {opcode}"
            );
        }
        assert_eq!(
            build_opplayer_packet(44, 0x1234, false),
            Some((117, [0x12, 0x34, 128]))
        );
        assert_eq!(
            build_opplayer_packet(2053, 0x0001, true),
            Some((91, [0, 1, 129]))
        );
        assert_eq!(build_opplayer_packet(23, 1, false), None);
        for outside in [43, 54, 2043, 2054, 0, 2000, 2055] {
            assert_eq!(
                build_opplayer_packet(outside, 1, false),
                None,
                "action {outside}"
            );
            assert_eq!(
                build_opplayer_packet(outside, 1, true),
                None,
                "action {outside}"
            );
        }
    }

    #[test]
    fn opplayert_packets_match_reference_layout() {
        // Action 16 (self index) and action 15 (other-player index) share one
        // `OPPLAYERT` (41) wire: activeId hi, lo; ctrl byte `v + 128`;
        // invobject lo, hi; playerIndex `lo + 128`, hi; parent as the bytes
        // `b8, b0, b24, b16`.
        let want = Some((
            41,
            [
                0x22, 0x22, 128, 0x34, 0x12, 0xF8, 0x56, 0x03, 0x04, 0x01, 0x02,
            ],
        ));
        assert_eq!(
            build_opplayert(15, 0x5678, 0x2222, 0x1234, 0x01020304, false),
            want
        );
        // Action 16 sends the same bytes given the same index (the self
        // variant takes the local player's index; the builder takes the index
        // as input).
        assert_eq!(
            build_opplayert(16, 0x5678, 0x2222, 0x1234, 0x01020304, false),
            want
        );
        assert_eq!(
            build_opplayert(15, 0x5678, 0x2222, 0x1234, 0x01020304, true),
            Some((
                41,
                [0x22, 0x22, 129, 0x34, 0x12, 0xF8, 0x56, 0x03, 0x04, 0x01, 0x02]
            ))
        );
        // The 2000 deprioritised flag is stripped before matching.
        assert_eq!(
            build_opplayert(2015, 1, 0, 0, 0, false),
            build_opplayert(15, 1, 0, 0, 0, false)
        );
        assert_eq!(
            build_opplayert(2016, 1, 0, 0, 0, false),
            build_opplayert(16, 1, 0, 0, 0, false)
        );
        assert_eq!(build_opplayert(14, 1, 0, 0, 0, false), None);
        assert_eq!(build_opplayert(17, 1, 0, 0, 0, false), None);
        assert_eq!(build_opplayert(44, 1, 0, 0, 0, false), None);
    }
}

#[cfg(test)]
#[path = "ui_player_options_oracle.rs"]
mod oracle;
