use super::*;
use native910::{
    script::Operand,
    vm::{Host, InstructionContext},
};

fn context<'a>(command: &'a str, operand: &'a Operand) -> InstructionContext<'a> {
    InstructionContext {
        script_name: Some("chat-host-test"),
        script_id: None,
        event: None,
        pc: 0,
        command,
        operand,
        secondary: false,
        int_locals: &[],
    }
}

#[test]
fn chat_send_commands_use_the_chat_huffman_coder_and_packet_lengths() {
    let operand = Operand::Byte(0);
    let mut engine = {
        let mut engine = Engine::default();
        engine.account.staff_mod_level = 1;
        engine.configs.wordpack = Some(crate::wordpack::Huffman::from_lengths(&[1, 1]).unwrap());
        engine
    };
    let mut ints = Vec::new();
    let mut longs = Vec::new();
    let public = context("chat_sendpublic", &operand);
    engine
        .trap_context(&public, &mut ints, &mut vec!["\u{1}".into()], &mut longs)
        .unwrap();
    assert_eq!(
        engine.outgoing,
        vec![crate::proto::client::MESSAGE_PUBLIC, 4, 0, 0, 1, 0x80]
    );

    // "glow1:wave:" -> colour 9, CHATEFFECT1 (1); "red:shake:" -> 1, 3.
    for (text, colour, effect) in [("glow1:wave:\u{1}", 9, 1), ("RED:shake:\u{1}", 1, 3)] {
        engine.outgoing.clear();
        engine
            .trap_context(&public, &mut ints, &mut vec![text.into()], &mut longs)
            .unwrap();
        assert_eq!(
            engine.outgoing,
            vec![
                crate::proto::client::MESSAGE_PUBLIC,
                4,
                colour,
                effect,
                1,
                0x80
            ]
        );
    }

    engine.outgoing.clear();
    let private = context("chat_sendprivate", &operand);
    engine
        .trap_context(
            &private,
            &mut ints,
            &mut vec!["A".into(), "\u{1}".into()],
            &mut longs,
        )
        .unwrap();
    assert_eq!(
        engine.outgoing,
        vec![
            crate::proto::client::MESSAGE_PRIVATE,
            0,
            4,
            b'A',
            0,
            1,
            0x80
        ]
    );
}

#[test]
fn abuse_report_uses_the_send_snapshot_framing() {
    let operand = Operand::Byte(0);
    let mut engine = Engine::default();
    let mut ints = vec![1, 2];
    let mut objects = vec!["Target".into(), "Reason".into()];
    let mut longs = Vec::new();
    engine
        .trap_context(
            &context("chat_sendabusereport", &operand),
            &mut ints,
            &mut objects,
            &mut longs,
        )
        .unwrap();
    assert_eq!(
        engine.outgoing,
        vec![
            crate::proto::client::SEND_SNAPSHOT,
            16,
            b'T',
            b'a',
            b'r',
            b'g',
            b'e',
            b't',
            0,
            0,
            2,
            b'R',
            b'e',
            b'a',
            b's',
            b'o',
            b'n',
            0,
        ]
    );
}
