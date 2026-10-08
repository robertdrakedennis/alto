use super::menu_builder::{AttackMenu, NpcOps};
use super::*;
use native910::{
    script::Operand,
    vm::{Host, InstructionContext},
};

#[test]
fn worldmap_commands_follow_script_runner() {
    let mut engine = Engine::default();
    engine
        .world_map
        .borrow_mut()
        .map
        .areas
        .push(crate::minimap::AreaMetadata {
            id: 7,
            name: "surface".into(),
            map_name: "surface".into(),
            config_origin: (1 << 28) | (100 << 14) | 200,
            config_bounds: [1000, 1020, 2000, 2020],
            background: -1,
            active: true,
            config_zoom: 4,
            subareas: vec![[1, 90, 190, 110, 210, 1000, 2000, 1020, 2020]],
        });
    let run = |engine: &mut Engine, command: &str, mut ints: Vec<i32>| {
        let operand = Operand::Byte(0);
        let context = InstructionContext {
            script_name: Some("worldmap-test"),
            script_id: None,
            event: None,
            pc: 0,
            command,
            operand: &operand,
            secondary: false,
            int_locals: &[],
        };
        let mut objects = Vec::new();
        let result = engine
            .trap_context(&context, &mut ints, &mut objects, &mut Vec::new())
            .unwrap();
        (result, ints, objects)
    };
    assert_eq!(
        run(&mut engine, "worldmap_getcurrentmap", vec![]).0,
        Some(Value::Int(-1))
    );
    // worldmap_setmap_coord_override: map id then coord.
    run(
        &mut engine,
        "worldmap_setmap_coord_override",
        vec![7, (100 << 14) | 200],
    );
    assert_eq!(
        run(&mut engine, "worldmap_getcurrentmap", vec![]).0,
        Some(Value::Int(7))
    );
    // Levels at or above the subarea's are accepted.
    assert_eq!(
        run(
            &mut engine,
            "worldmap_getdisplaycoord",
            vec![(1 << 28) | (100 << 14) | 200]
        )
        .1,
        [1010, 2010]
    );
    assert_eq!(
        run(
            &mut engine,
            "worldmap_getdisplaycoord",
            vec![(2 << 28) | (100 << 14) | 200]
        )
        .1,
        [1010, 2010]
    );
    assert_eq!(
        run(
            &mut engine,
            "worldmap_getdisplaycoord",
            vec![(100 << 14) | 200]
        )
        .1,
        [-1, -1]
    );
    // The loading step 10: origin, size and the requested
    // source coordinate (no player).
    engine
        .world_map
        .borrow_mut()
        .update_loading(None, &|_| true);
    {
        let wm = engine.world_map.borrow();
        assert_eq!(
            wm.map.area.as_ref().map(|a| (a.origin, a.size)),
            Some(([960, 1984], [64, 64]))
        );
        assert_eq!(wm.position, [50, 26]);
        assert_eq!(wm.loading, 20);
    }
    assert_eq!(
        run(&mut engine, "worldmap_getdisplayposition", vec![]).1,
        [1010, 2010]
    );
    assert_eq!(
        run(&mut engine, "worldmap_isloaded", vec![]).0,
        Some(Value::Int(0))
    );
    run(
        &mut engine,
        "worldmap_jumptodisplaycoord_instant",
        vec![(1011 << 14) | 2011],
    );
    assert_eq!(
        run(&mut engine, "worldmap_getdisplayposition", vec![]).1,
        [1011, 2011]
    );
    assert_eq!(
        run(&mut engine, "worldmap_getsourceposition", vec![]).1,
        [101, 201]
    );
    run(
        &mut engine,
        "worldmap_jumptosourcecoord_instant",
        vec![(1 << 28) | (100 << 14) | 200],
    );
    assert_eq!(
        run(&mut engine, "worldmap_getdisplayposition", vec![]).1,
        [1010, 2010]
    );
    assert_eq!(
        run(
            &mut engine,
            "worldmap_getsourcecoord",
            vec![(1010 << 14) | 2010]
        )
        .1,
        [100, 200]
    );
    assert_eq!(
        run(&mut engine, "worldmap_getconfigsize", vec![7]).1,
        [20, 20]
    );
    // The config bounds (x0, x1, z0, z1).
    assert_eq!(
        run(&mut engine, "worldmap_getconfigbounds", vec![7]).1,
        [1000, 2000, 1020, 2020]
    );
    assert_eq!(
        run(&mut engine, "worldmap_getconfigzoom", vec![7]).0,
        Some(Value::Int(4))
    );
    assert_eq!(
        run(&mut engine, "worldmap_getconfigorigin", vec![7]).0,
        Some(Value::Int((1 << 28) | (100 << 14) | 200))
    );
    assert_eq!(
        run(
            &mut engine,
            "worldmap_coordinmap",
            vec![(100 << 14) | 200, 7]
        )
        .0,
        Some(Value::Int(1))
    );
    assert_eq!(
        run(&mut engine, "worldmap_getmap", vec![(100 << 14) | 200]).0,
        Some(Value::Int(7))
    );
    assert_eq!(
        run(&mut engine, "worldmap_getmapname", vec![7]).2,
        ["surface"]
    );
    run(&mut engine, "worldmap_setzoom", vec![75]);
    assert_eq!(
        run(&mut engine, "worldmap_getzoom", vec![]).0,
        Some(Value::Int(75))
    );
    run(&mut engine, "worldmap_disableelementcategory", vec![42, 1]);
    assert_eq!(
        run(&mut engine, "worldmap_getdisableelementcategory", vec![42]).0,
        Some(Value::Int(1))
    );
    // Elements: listing needs the listing flag and the variable test.
    {
        let mut wm = engine.world_map.borrow_mut();
        wm.element_types
            .types
            .insert(42, crate::minimap::MapElement::decode(&[0]).unwrap());
        let area = wm.map.area.as_mut().unwrap();
        area.elements = vec![
            crate::world_map::Element::new(42, 50, 26),
            crate::world_map::Element::new(42, 51, 27),
        ];
    }
    assert_eq!(
        run(&mut engine, "worldmap_listelement_start", vec![]).1,
        [42, (1010 << 14) | 2010]
    );
    assert_eq!(
        run(&mut engine, "worldmap_listelement_next", vec![]).1,
        [42, (1011 << 14) | 2011]
    );
    assert_eq!(
        run(&mut engine, "worldmap_listelement_next", vec![]).1,
        [-1, -1]
    );
    // getNearestElement answers -2 (-> -1) until loading reaches 100.
    assert_eq!(
        run(
            &mut engine,
            "worldmap_findnearestelement",
            vec![42, (1011 << 14) | 2012]
        )
        .0,
        Some(Value::Int(-1))
    );
    engine.world_map.borrow_mut().loading = 100;
    assert_eq!(
        run(
            &mut engine,
            "worldmap_findnearestelement",
            vec![42, (1011 << 14) | 2012]
        )
        .0,
        Some(Value::Int((1011 << 14) | 2011))
    );
}

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

#[test]
fn npc_menu_names_follow_the_level_text() {
    assert_eq!(npc_menu_name("Man".into(), 0, 50), "Man");
    assert_eq!(
        npc_menu_name("Man".into(), 2, 50),
        "Man<col=ff00> (level: 2)"
    );
    assert_eq!(
        npc_menu_name("Guard".into(), 55, 50),
        "Guard<col=ff7000> (level: 55)"
    );
    // vislevel defaults to -1 and the original client only hides 0.
    assert_eq!(
        npc_menu_name("Rat".into(), -1, 3),
        "Rat<col=80ff00> (level: -1)"
    );
}

#[test]
fn npc_menu_cursors_follow_the_default_type_and_attack_order() {
    use crate::ui_player_options::AttackPriority as P;
    let decoded = [
        Some("Talk".to_string()),
        Some("Attack".to_string()),
        None,
        None,
        None,
    ];
    let ops = npc_type_ops(decoded.each_ref().map(Option::as_deref));
    // defaultMenuCursor 50; getCursor(0)=3, getCursor(5)=8; Attack takes
    // cursorattack unconditionally, even -1 (addOption's defaultCursor).
    let cursors = [3, 4, -1, -1, -1, 8];
    assert_eq!(
        npc_menu_operation_slots(
            NpcOps {
                ops: &ops,
                op_mask: 0,
                reprioritise: 0,
                vislevel: 1,
                cursors: &cursors,
                cursor_attack: -1
            },
            AttackMenu {
                priority: P::Default,
                local_combat: 1,
                default_menu_cursor: 50
            }
        ),
        vec![(5, 1003, 8), (1, 10, -1), (0, 9, 3)]
    );
    assert_eq!(
        npc_menu_operation_slots(
            NpcOps {
                ops: &ops,
                op_mask: 0,
                reprioritise: 1,
                vislevel: 1,
                cursors: &cursors,
                cursor_attack: 77
            },
            AttackMenu {
                priority: P::Default,
                local_combat: 1,
                default_menu_cursor: 50
            }
        ),
        vec![(0, 9, 3), (1, 10, 77), (5, 1003, 8)]
    );
}
