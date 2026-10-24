use super::*;

fn wire(opcode: u8, payload: &[u8]) -> Vec<u8> {
    // Opcodes >= 128 use the two-byte form.
    let mut out = if opcode < 128 {
        vec![opcode]
    } else {
        vec![128, opcode]
    };
    match crate::proto::server::size(opcode).unwrap() {
        -2 => out.extend((payload.len() as u16).to_be_bytes()),
        -1 => out.push(payload.len() as u8),
        n => assert_eq!(n as usize, payload.len()),
    }
    out.extend(payload);
    out
}

fn sync(opcode: u8, payload: &[u8]) -> (Vec<u8>, Vec<UiEvent>) {
    let bytes = wire(opcode, payload);
    let (frame, used) = crate::net::decode_frame(&bytes).unwrap().unwrap();
    assert_eq!(used, bytes.len());
    let (mut writes, mut out) = (Vec::new(), Vec::new());
    assert!(handle_sync_frame(frame, &mut writes, &mut out)
        .unwrap()
        .is_none());
    (writes, out)
}

#[test]
fn send_ping_reply_matches_alt_encodings() {
    // p4_alt1(0x01020304), p4_alt3(0x0a0b0c0d), p1_alt2(50).
    assert_eq!(
        crate::net::encode_send_ping_reply(0x0102_0304, 0x0a0b_0c0d, 50),
        [100, 4, 3, 2, 1, 0x0b, 0x0a, 0x0d, 0x0c, 206]
    );
    let mut payload = 0x0102_0304i32.to_be_bytes().to_vec();
    payload.extend(0x0a0b_0c0di32.to_be_bytes());
    let (writes, events) = sync(crate::proto::server::SEND_PING, &payload);
    assert!(events.is_empty());
    assert_eq!(writes.len(), 10);
    assert_eq!(writes[..9], [100, 4, 3, 2, 1, 0x0b, 0x0a, 0x0d, 0x0c]);
}

#[test]
fn client_shell_packets_decode_to_their_owners() {
    use crate::proto::server as p;
    assert_eq!(
        sync(p::EXECUTE_CLIENT_CHEAT, &[0, 29]).1,
        [UiEvent::ExecuteClientCheat { id: 29 }]
    );
    assert_eq!(
        sync(p::DO_CHEAT, b"cls\0").1,
        [UiEvent::DoCheat {
            command: "cls".into()
        }]
    );
    assert_eq!(sync(p::JS5_RELOAD, &[]).1, [UiEvent::Js5Reload]);
    assert_eq!(
        sync(p::UNHANDLED_51, &[1, 2]).1,
        [UiEvent::UnhandledPacket {
            opcode: 51,
            size: 2
        }]
    );
    assert_eq!(
        sync(p::UNHANDLED_170, &[]).1,
        [UiEvent::UnhandledPacket {
            opcode: 170,
            size: 0
        }]
    );
    assert_eq!(
        sync(p::DEBUG_SERVER_TRIGGERS, &[0, 7, 0, 1, 0, 2, 0xff, 0, 1]).1,
        [UiEvent::DebugServerTriggers {
            interface: 7,
            start: 1,
            end: 2,
            values: vec![0xff_0001],
        }]
    );
    let mut reflection = vec![1];
    reflection.extend(77i32.to_be_bytes());
    reflection.extend([2]);
    reflection.extend(b"client\0f\0");
    let (_, events) = sync(p::REFLECTION_CHECKER, &reflection);
    let [UiEvent::ReflectionProbe(check)] = events.as_slice() else {
        panic!("{events:?}");
    };
    assert_eq!(check.id, 77);
    assert_eq!(check.entries[0].status, -1);
    assert!(parse_ui_event(p::EXECUTE_CLIENT_CHEAT, &[0]).is_err());
    assert!(parse_ui_event(p::DO_CHEAT, b"x").is_err());
}

#[test]
fn midi_song_location_keeps_signed_level_and_absolute_tiles() {
    // g4_alt3(0x90004005), g4s(7), g1_alt1(2), g1(200), g1_alt2(3).
    let payload = [0x00, 0x90, 0x05, 0x40, 0, 0, 0, 7, 130, 200, 253];
    assert_eq!(
        parse_ui_event(crate::proto::server::MIDI_SONG_LOCATION, &payload).unwrap(),
        Some(UiEvent::Audio {
            command: "midi_song_location".into(),
            args: vec![7, 200, -7, 1, 5, 2, 3],
        })
    );
}
