use super::menu_builder::{AttackMenu, NpcOps};
use super::*;

#[test]
fn npc_menu_attack_reprioritisation_matches_the_two_pass_order() {
    use crate::ui_player_options::AttackPriority as P;
    // op after defaultops: slot 5 is Examine.
    let decoded = [
        Some("Talk".to_string()),
        Some("Attack".to_string()),
        None,
        None,
        None,
    ];
    let ops = npc_type_ops(decoded.each_ref().map(Option::as_deref));
    assert_eq!(ops[5], Some("Examine"));
    let cursors = [-1; 6];
    let slots = |reprioritise, priority, vislevel| {
        npc_menu_operation_slots(
            NpcOps {
                ops: &ops,
                op_mask: 0,
                reprioritise,
                vislevel,
                cursors: &cursors,
                cursor_attack: -1,
            },
            AttackMenu {
                priority,
                local_combat: 20,
                default_menu_cursor: -1,
            },
        )
        .into_iter()
        .map(|(slot, action, _)| (slot, action))
        .collect::<Vec<_>>()
    };
    // addNPCEntries traced by hand for each policy.
    assert_eq!(slots(1, P::Default, 10), vec![(0, 9), (1, 10), (5, 1003)]);
    assert_eq!(slots(0, P::Default, 10), vec![(5, 1003), (1, 10), (0, 9)]);
    assert_eq!(
        slots(0, P::AlwaysRight, 10),
        vec![(0, 9), (1, 2010), (5, 3003)]
    );
    assert_eq!(slots(0, P::Hidden, 10), vec![(5, 1003), (0, 9)]);
    assert_eq!(
        slots(1, P::HigherLevelRight, 30),
        vec![(0, 9), (1, 2010), (5, 3003)]
    );
    assert_eq!(
        slots(1, P::HigherLevelRight, 10),
        vec![(0, 9), (1, 10), (5, 1003)]
    );
    // Server op mask bit 1 removes Attack only.
    assert_eq!(
        npc_menu_operation_slots(
            NpcOps {
                ops: &ops,
                op_mask: 2,
                reprioritise: 1,
                vislevel: 10,
                cursors: &cursors,
                cursor_attack: -1
            },
            AttackMenu {
                priority: P::Default,
                local_combat: 20,
                default_menu_cursor: -1
            }
        )
        .into_iter()
        .map(|(slot, action, _)| (slot, action))
        .collect::<Vec<_>>(),
        vec![(0, 9), (5, 1003)]
    );
}
