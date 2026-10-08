use crate::ui_runtime::*;
use native910::vm::Host;

#[test]
fn direct_engine_minimenu_queries_read_runtime_snapshot() {
    let mut engine = {
        let mut engine = Engine::default();
        engine.menu.active = crate::ui_minimenu::EntryView {
            entity_type: 4,
            op: "Examine".into(),
            op_base: "Goblin".into(),
            quest_text: Some(" <sprite=7>".into()),
        };
        engine.menu.secondary = crate::ui_minimenu::EntryView {
            entity_type: 0,
            op: String::new(),
            op_base: String::new(),
            quest_text: Some(String::new()),
        };
        engine.menu.counts = [3, 1];
        engine
    };
    let operand = native910::script::Operand::Byte(0);
    let context = native910::vm::InstructionContext {
        script_name: Some("direct-minimenu"),
        script_id: None,
        event: None,
        pc: 0,
        command: "get_active_minimenu_entry",
        operand: &operand,
        secondary: false,
        int_locals: &[],
    };
    let mut ints = Vec::new();
    let mut objs = Vec::new();
    engine
        .trap_context(&context, &mut ints, &mut objs, &mut Vec::new())
        .unwrap();
    assert_eq!(ints, vec![4]);
    assert_eq!(objs, vec!["Examine", "Goblin", " <sprite=7>"]);
    let length = native910::vm::InstructionContext {
        command: "get_minimenu_length",
        ..context
    };
    engine
        .trap_context(&length, &mut ints, &mut objs, &mut Vec::new())
        .unwrap();
    assert_eq!(&ints[1..], &[3, 1]);
}
