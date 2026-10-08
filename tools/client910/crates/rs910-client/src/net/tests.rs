use super::*;

/// `LoginParams` for the payload tests: `username`/`password` with the
/// given site settings, uid.dat and auth options.
fn params(
    username: &str,
    password: &str,
    site_settings: &str,
    uid192: &[u8; 24],
    auth: &AuthOptions,
) -> LoginParams {
    LoginParams {
        site_settings: site_settings.to_owned(),
        uid192: *uid192,
        auth: auth.clone(),
        ..LoginParams::new("127.0.0.1", 0, username, password)
    }
}

/// `username`/`password` with the defaults of [`LoginParams::new`].
fn creds(username: &str, password: &str) -> LoginParams {
    LoginParams::new("127.0.0.1", 0, username, password)
}

/// The step-98 reply policy of a first attempt: nothing captured, no social
/// key to lose, and the lobby's login waits for the player.
fn policy(where_: &str, is_lobby: bool) -> ReplyPolicy<'_> {
    ReplyPolicy {
        where_,
        is_lobby,
        holds: is_lobby,
        can_retry_world_full: true,
        capture_disallow: false,
        renegotiates: false,
    }
}

/// A `loginwait` expiry is the timeout path,
/// distinct from an I/O error, even under read context.
#[tokio::test]
async fn loginwait_expiry_is_classified_as_timeout() {
    let (mut client, _server) = tokio::io::duplex(64);
    let clock = LoginClock {
        started: std::time::Instant::now(),
        accumulated: Duration::ZERO,
        limit_ticks: 0,
        paused: false,
        social: false,
    };
    let mut buf = [0u8; 1];
    let error = read_exact_clock(&mut client, &mut buf, &clock, "reply")
        .await
        .unwrap_err()
        .context("outer");
    let Attempt::<()>::RetryAfter { timeout, .. } = retry_read(&error, "x".into()) else {
        panic!("retry expected");
    };
    assert!(timeout);
    let (mut client, server) = tokio::io::duplex(64);
    drop(server);
    let error = read_exact_clock(&mut client, &mut buf, &LoginClock::new(true), "reply")
        .await
        .unwrap_err();
    let Attempt::<()>::RetryAfter { timeout, .. } = retry_read(&error, "x".into()) else {
        panic!("retry expected");
    };
    assert!(!timeout, "EOF is the I/O error path");
    assert_eq!(LoginAttemptsExhausted::from_last_attempt(true).reply, -5);
    assert_eq!(LoginAttemptsExhausted::from_last_attempt(false).reply, -4);
}

/// Four refused connects exhaust the
/// attempts through the I/O error path and report -4.
#[tokio::test]
async fn refused_connects_exhaust_with_reply_minus_four() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let error = login_lobby(&LoginParams::new("127.0.0.1", port, "u", "p"))
        .await
        .expect_err("refused");
    assert_eq!(
        error.downcast_ref::<LoginAttemptsExhausted>(),
        Some(&LoginAttemptsExhausted { reply: -4 })
    );
}

fn be_i32(bytes: &[u8]) -> i32 {
    i32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn be_i64(bytes: &[u8]) -> i64 {
    i64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

#[test]
fn lobby_packet_framing_and_leading_words() {
    let packet = build_lobby_login_packet(&creds("test", "secret"), 0x0102030405060708).unwrap();
    assert_eq!(packet[0], 19);
    let len = u16::from_be_bytes([packet[1], packet[2]]) as usize;
    assert_eq!(len, packet.len() - 3, "u16 size must equal payload length");
    let body = &packet[3..];
    assert_eq!(be_i32(&body[0..4]), 910);
    assert_eq!(be_i32(&body[4..8]), 1);
    // RSA-stub: marker 10, zeroed seeds, echoed server token.
    assert_eq!(body[8], 10);
    assert_eq!(&body[9..25], &[0u8; 16]);
    assert_eq!(be_i64(&body[25..33]), 0x0102030405060708);
    // TOTP AUTH_NOT_FOUND + reserved word, pbool(false), password jstr.
    assert_eq!(body[33], 2);
    assert_eq!(&body[34..38], &[0u8; 4]);
    assert_eq!(body[38], 0);
    assert_eq!(&body[39..46], b"secret\x00");
    assert_eq!(&body[46..54], &[0u8; 8]); // socialname
    assert_eq!(&body[54..62], &[0u8; 8]); // unused 64-bit login field
                                          // Tiny-stub: string-marker + username jstr.
    assert_eq!(body[62], 1);
    assert_eq!(&body[63..68], b"test\x00");
}

/// The window and the saved options reach both login packets: the mode, the
/// canvas, the anti-aliasing level and the options block.
#[test]
fn logins_report_the_window_and_the_options() {
    let mut params = creds("a", "b");
    params.client = ClientReport {
        window_mode: 3,
        canvas: [1920, 1200],
        anti_aliasing: 2,
        preferences: (0..58).collect(),
    };
    for packet in [
        build_lobby_login_packet(&params, 0).unwrap(),
        build_game_login_packet(&params, 0).unwrap(),
    ] {
        let body = &packet[3..];
        // The options block is a length byte and 58 bytes; the site string
        // (empty), the 24-byte uid and the window fields sit before it. The
        // game login's affiliate word sits between them.
        let at = body
            .windows(59)
            .position(|w| w[0] == 58 && w[1..].iter().copied().eq(0..58u8))
            .expect("the options block");
        let game = packet[0] == crate::proto::login::GAMELOGIN;
        let before = at - if game { 4 } else { 0 } - 1 - 24 - 6;
        assert_eq!(body[before], 3, "window mode");
        assert_eq!(&body[before + 1..before + 5], &[0x07, 0x80, 0x04, 0xB0]);
        assert_eq!(body[before + 5], 2, "anti-aliasing");
    }
    // Unreported clients keep the default report.
    let default = build_lobby_login_packet(&creds("a", "b"), 0).unwrap();
    assert!(default.windows(3).all(|w| w != [3, 0x07, 0x80]));
}

/// A probed machine reaches the login block field by field, and the
/// all-unknown machine keeps the 53-byte block with zero fields.
#[test]
fn hardware_block_carries_the_probed_machine() {
    let machine = Hardware {
        os_id: 2,
        os_64bit: true,
        os_version_code: 27,
        memory_budget_mb: 4096,
        logical_cpus: 10,
        ram_mb: 16_384,
        cpu_mhz: 3200,
        gpu_description: "Test GPU".into(),
        cpu_vendor: "TestVendor".into(),
        cpu_description: "Test CPU".into(),
        cpu_logical_per_package: 2,
        cpu_signature: 0x0a0b_0c0d,
        cpu_features: [1, 2, 3],
    };
    let mut writer = ByteWriter::default();
    push_hardware_block(&mut writer, &machine).unwrap();
    let b = writer.data;
    assert_eq!(&b[..3], &[8, 2, 1]);
    assert_eq!(&b[3..5], &27u16.to_be_bytes());
    // Runtime vendor and versions, then the unused flag.
    assert_eq!(&b[5..10], &[0; 5]);
    assert_eq!(&b[10..12], &4096u16.to_be_bytes());
    assert_eq!(b[12], 10);
    assert_eq!(&b[13..16], &[0, 0x40, 0x00]); // 16384 as 3 bytes
    assert_eq!(&b[16..18], &3200u16.to_be_bytes());
    // gpu description: version byte, text, NUL.
    assert_eq!(&b[18..28], b"\0Test GPU\0");
    let tail = &b[b.len() - 2 - 16 - 2..];
    assert_eq!(&tail[..2], &[10, 2]);
    assert_eq!(&tail[2..6], &1i32.to_be_bytes());
    assert_eq!(&tail[6..10], &2i32.to_be_bytes());
    assert_eq!(&tail[10..14], &3i32.to_be_bytes());
    assert_eq!(&tail[14..18], &0x0a0b_0c0di32.to_be_bytes());
    let mut unknown = ByteWriter::default();
    push_hardware_block(&mut unknown, &Hardware::default()).unwrap();
    assert_eq!(unknown.data.len(), 53);
    assert_eq!(&unknown.data[..3], &[8, 0, 0]);
}

#[test]
fn lobby_payload_tail_shapes() {
    let packet = build_lobby_login_packet(&creds("a", "b"), 0).unwrap();
    let body = &packet[3..];
    // prefs stub: length byte 58 + 58 zero bytes.
    let prefs_at = body.len() - (1 + 58 + 53 + 4 + 1 + 4 + 4 + 1 + 1 + 1 + 41 * 4);
    assert_eq!(body[prefs_at], 58);
    assert_eq!(&body[prefs_at + 1..prefs_at + 59], &[0u8; 58]);
    // hardware stub: 53 bytes starting with marker 8.
    let hw = &body[prefs_at + 59..prefs_at + 59 + 53];
    assert_eq!(hw.len(), 53);
    assert_eq!(hw[0], 8);
    // tail: verify + unused string("") + affiliate + unused int + gamepack("")
    // + clientType + pbool + 41 crcs.
    let tail = &body[prefs_at + 59 + 53..];
    assert_eq!(tail.len(), 4 + 1 + 4 + 4 + 1 + 1 + 1 + 41 * 4);
    assert_eq!(&tail[0..4], &[0u8; 4]);
    assert_eq!(tail[4], 0);
    assert_eq!(&tail[5..9], &[0u8; 4]);
    assert_eq!(&tail[9..13], &[0u8; 4]);
    assert_eq!(tail[13], 0);
    assert_eq!(tail[14], 0);
    assert_eq!(tail[15], 0);
    assert_eq!(&tail[16..], &[0u8; 41 * 4]);
}

#[test]
fn game_packet_framing_and_state_byte() {
    let packet = build_game_login_packet(&creds("test", "secret"), 0x1112131415161718).unwrap();
    assert_eq!(packet[0], 16);
    let len = u16::from_be_bytes([packet[1], packet[2]]) as usize;
    assert_eq!(len, packet.len() - 3);
    let body = &packet[3..];
    assert_eq!(be_i32(&body[0..4]), 910);
    assert_eq!(be_i32(&body[4..8]), 1);
    assert_eq!(body[8], 0, "fresh login: state != 14, no hop key block");
    assert_eq!(body[9], 10, "RSA-stub marker follows the state byte");
    assert_eq!(be_i64(&body[26..34]), 0x1112131415161718);
    let trusted = AuthOptions {
        new_auth_preference: "123456".into(),
        auth_dont_trust: false,
        ..AuthOptions::default()
    };
    // Reconnect (client state 14): header byte 1 and outKey2 after the token.
    let reconnect = build_game_login_packet(
        &params(
            "test",
            "secret",
            "",
            &[0; 24],
            &AuthOptions {
                reconnect: true,
                ..AuthOptions::default()
            },
        ),
        0x1112131415161718,
    )
    .unwrap();
    let reconnect_body = &reconnect[3..];
    assert_eq!(reconnect_body[8], 1, "reconnect: header byte 1");
    assert_eq!(be_i64(&reconnect_body[26..34]), 0x1112131415161718);
    assert_eq!(&reconnect_body[34..50], &[0; 16], "outKey2 seeds");
    assert_eq!(
        (reconnect_body[50], &reconnect_body[51..56]),
        (1, &b"test\0"[..]),
        "a reconnect carries no credentials: the username follows outKey2"
    );
    let trusted_packet = build_game_login_packet(
        &params("test", "secret", "", &[0; 24], &trusted),
        0x1112131415161718,
    )
    .unwrap();
    let trusted_body = &trusted_packet[3..];
    assert_eq!(trusted_body[34], 3, "trust-this-computer type id");
    assert_eq!(&trusted_body[35..38], &[1, 226, 64]);
    assert_eq!(trusted_body[38], 0);
}

/// The RSA block of the original client's login, recorded once for each
/// authenticator case (trust and don't-trust codes, none, and a reconnect with
/// and without a code), equals the block both login builders write.
#[test]
fn rsa_block_matches_the_recording() {
    let recording = rs910_core::test_support::frozen::text("login-auth/rsa-blocks.txt");
    let mut seen = Vec::new();
    for line in recording.lines() {
        let (name, hex) = line.split_once(' ').unwrap();
        let block: Vec<u8> = (0..hex.len() / 2)
            .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
            .collect();
        let (new_auth_preference, auth_dont_trust) = match name {
            "trust" => ("123456", false),
            "dont-trust" | "reconnect-with-preference" => ("654321", true),
            "short-preference" => ("12345", false),
            _ => ("", false),
        };
        let reconnect = name.starts_with("reconnect");
        let login = params(
            "user",
            "p\u{e9}ss",
            "",
            &[0; 24],
            &AuthOptions {
                new_auth_preference: new_auth_preference.into(),
                auth_dont_trust,
                reconnect,
            },
        );
        let token = 0x0A0B_0C0D_0E0F_1011;
        // Both blocks are followed by the username marker and the username.
        let tail = b"\x01user\0";
        let game = game_login_payload(&login, token).unwrap();
        assert_eq!(&game[9..9 + block.len()], &block[..], "{name}: game login");
        assert_eq!(
            &game[9 + block.len()..9 + block.len() + tail.len()],
            tail,
            "{name}: game login username"
        );
        if !reconnect {
            let lobby = lobby_login_payload(&login, token).unwrap();
            assert_eq!(
                &lobby[8..8 + block.len()],
                &block[..],
                "{name}: lobby login"
            );
        }
        seen.push(name);
    }
    assert_eq!(
        seen,
        [
            "not-found",
            "trust",
            "dont-trust",
            "short-preference",
            "reconnect",
            "reconnect-with-preference"
        ]
    );
}

fn game_login_variants_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/game_login_variants.json")
}

/// The GAMELOGIN payloads the dev server's tests decode: a fresh login, both
/// authenticator-code forms and a reconnect, all for "Alice" with token 7.
fn game_login_variants_json() -> String {
    let cases = [
        ("fresh", "", false, false, false),
        ("trust", "123456", false, false, false),
        ("dont-trust", "654321", true, false, false),
        ("reconnect", "", false, true, false),
        ("launcher", "", false, false, true),
    ];
    let rows: Vec<_> = cases
        .iter()
        .map(|&(name, code, dont_trust, reconnect, launcher)| {
            let auth = AuthOptions {
                new_auth_preference: code.into(),
                auth_dont_trust: dont_trust,
                reconnect,
            };
            let mut login = params("Alice", "alice-password", "", &[0; 24], &auth);
            if launcher {
                // Every launcher parameter, a running verify counter and
                // archive checksums, as a launched client reports them.
                login.launcher = LauncherReport {
                    language: 1,
                    affiliate: 17,
                    user_flow: [5, 3],
                    flags: [7, 9],
                    label: "abc".into(),
                    additional_info: Some("extra".into()),
                    javascript: true,
                    chrome: true,
                    client_type: 14561,
                    build: 1_449_949_008,
                    gamepack: "pack".into(),
                    ..LauncherReport::default()
                };
                login.verify_id = 0x0102_0304;
                login.archive_checksums = std::array::from_fn(|word| 0x1000_0000 + word as i32);
            }
            let payload = game_login_payload(&login, 7).unwrap();
            serde_json::json!({
                "name": name,
                "username": "Alice",
                "token": "7",
                "reconnect": reconnect,
                "hex": payload.iter().map(|b| format!("{b:02x}")).collect::<String>(),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "about": "GAMELOGIN payloads the client builds (net::game_login_payload); decoded by server/src/lostcity/network/GameLogin.test.ts and replayed by SocketE2E.integration.test.ts.",
        "regenerate": "cd tools/client910 && cargo test --lib net::tests::regenerate_game_login_variants -- --ignored",
        "cases": rows,
    });
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

/// The committed payloads are what the builder writes today, so the server
/// tests that decode them follow the client.
#[test]
fn game_login_variants_fixture_is_current() {
    let fixture = std::fs::read_to_string(game_login_variants_path()).unwrap();
    assert_eq!(
        fixture,
        game_login_variants_json(),
        "fixture is stale; regenerate it (see its `regenerate` field)"
    );
}

#[test]
#[ignore = "fixture regeneration"]
fn regenerate_game_login_variants() {
    std::fs::write(game_login_variants_path(), game_login_variants_json()).unwrap();
}

#[test]
fn site_settings_are_carried_in_lobby_and_game_payloads() {
    let expected = b"site=foo\0";
    let lobby = lobby_login_payload(
        &params("user", "pw", "site=foo", &[0; 24], &AuthOptions::default()),
        0,
    )
    .unwrap();
    let game = game_login_payload(
        &params("user", "pw", "site=foo", &[0; 24], &AuthOptions::default()),
        0,
    )
    .unwrap();
    assert!(lobby
        .windows(expected.len())
        .any(|window| window == expected));
    assert!(game
        .windows(expected.len())
        .any(|window| window == expected));
    let uid: [u8; 24] = std::array::from_fn(|index| index as u8 + 1);
    let lobby_with_uid = build_lobby_login_packet(
        &params("user", "pw", "site=foo", &uid, &AuthOptions::default()),
        0,
    )
    .unwrap();
    let game_with_uid = build_game_login_packet(
        &params("user", "pw", "site=foo", &uid, &AuthOptions::default()),
        0,
    )
    .unwrap();
    assert!(lobby_with_uid
        .windows(uid.len())
        .any(|window| window == uid));
    assert!(game_with_uid.windows(uid.len()).any(|window| window == uid));
    // The uid block: an
    // absent (all-zero) uid.dat goes out as 24 x -1, directly before the
    // site settings string.
    let absent: Vec<u8> = [0xFF; 24].iter().chain(expected).copied().collect();
    for packet in [
        build_lobby_login_packet(
            &params("user", "pw", "site=foo", &[0; 24], &AuthOptions::default()),
            0,
        )
        .unwrap(),
        build_game_login_packet(
            &params("user", "pw", "site=foo", &[0; 24], &AuthOptions::default()),
            0,
        )
        .unwrap(),
    ] {
        assert!(packet.windows(absent.len()).any(|window| window == absent));
    }
}

#[test]
fn client_sender_vectors_are_byte_exact() {
    // Bare-opcode handshake (14) and game-login continue (26).
    assert_eq!(init_game_connection_packet(), vec![14]);
    assert_eq!(game_login_continue_packet(), vec![26]);
    assert_eq!(encode_no_timeout(), vec![103]);
    assert_eq!(
        encode_map_build_complete(0x01020304),
        vec![79, 0x01, 0x02, 0x03, 0x04]
    );
    assert_eq!(
        encode_window_status(2, 800, 600, 1),
        vec![123, 2, 0x03, 0x20, 0x02, 0x58, 1]
    );
    assert_eq!(
        encode_worldlist_fetch(0x0A0B0C0D),
        vec![77, 0x0A, 0x0B, 0x0C, 0x0D]
    );
}

/// Every server opcode of the protocol table, `(opcode, size)` in opcode
/// order (the table is dense, opcodes 0 to 194).
fn server_table() -> Vec<(i32, i32)> {
    (0..crate::proto::server::COUNT as u8)
        .map(|id| (i32::from(id), crate::proto::server::size(id).unwrap()))
        .collect()
}

/// Every opcode of the table, eight times with varying payload lengths, as one
/// byte stream (the input of the frozen framing recording).
fn framing_wire() -> Vec<u8> {
    const SHORT_LENGTHS: [i32; 8] = [0, 1, 2, 127, 128, 254, 255, 17];
    const LONG_LENGTHS: [i32; 8] = [0, 1, 2, 255, 256, 4096, 32768, 65535];
    let table = server_table();
    let mut wire = Vec::new();
    for pass in 0..8usize {
        for &(id, size) in &table {
            if id < 128 {
                wire.push(id as u8);
            } else {
                wire.extend(((id + 32768) as u16).to_be_bytes());
            }
            let n = match size {
                -1 => SHORT_LENGTHS[pass],
                -2 => LONG_LENGTHS[pass],
                fixed => fixed,
            };
            match size {
                -1 => wire.push(n as u8),
                -2 => wire.extend((n as u16).to_be_bytes()),
                _ => {}
            }
            for k in 0..n {
                wire.push(((id * 17 + k * 13 + pass as i32) & 255) as u8);
            }
        }
    }
    wire
}

/// Exhaustive table and byte fragmentation against the frozen recording of
/// the original client's frame reader.
#[tokio::test]
async fn real_client_framing_replay() -> Result<()> {
    let wire = framing_wire();
    let mut pending = Vec::new();
    let mut state = ResyncState::default();
    let mut result = Vec::new();
    for (i, &byte) in wire.iter().enumerate() {
        pending.push(byte);
        while let Some((frame, used)) = decode_frame_at(&pending, &state)? {
            state.advance(used as u64, frame.opcode);
            for v in [
                (i + 1) as u32,
                state.offset as u32,
                frame.opcode as u32,
                frame.payload.len() as u32,
            ] {
                result.extend(v.to_be_bytes());
            }
            result.extend(&frame.payload);
            pending.drain(..used);
        }
    }
    anyhow::ensure!(pending.is_empty(), "partial wire tail");
    assert_eq!(state.offset as usize, wire.len());
    rs910_core::test_support::frozen::assert_stream("framing/recording", &result);
    Ok(())
}

#[test]
fn decode_frame_vectors() {
    // Fixed-size server frame: NO_TIMEOUT (83/0).
    let (frame, used) = decode_frame(&[83]).unwrap().unwrap();
    assert_eq!(
        frame,
        Frame {
            opcode: 83,
            payload: vec![]
        }
    );
    assert_eq!(used, 1);
    // Fixed-size: UPDATE_ZONE_PARTIAL_FOLLOWS (56/3).
    let (frame, used) = decode_frame(&[56, 9, 9, 9]).unwrap().unwrap();
    assert_eq!(frame.opcode, 56);
    assert_eq!(frame.payload, vec![9, 9, 9]);
    assert_eq!(used, 4);
    // Variable u16: REBUILD_NORMAL (88/-2) with 5-byte payload.
    let (frame, used) = decode_frame(&[88, 0, 5, 1, 2, 3, 4, 5]).unwrap().unwrap();
    assert_eq!(frame.opcode, 88);
    assert_eq!(frame.payload, vec![1, 2, 3, 4, 5]);
    assert_eq!(used, 8);
    // Two back-to-back frames consume exactly.
    let bytes = [128, 129, 83];
    let (first, used) = decode_frame(&bytes).unwrap().unwrap();
    assert_eq!(first.opcode, 129);
    let (second, used2) = decode_frame(&bytes[used..]).unwrap().unwrap();
    assert_eq!(second.opcode, 83);
    assert_eq!(used + used2, 3);
}

#[test]
fn decode_frame_incomplete_and_unknown() {
    assert!(decode_frame(&[]).unwrap().is_none());
    assert!(decode_frame(&[88, 0]).unwrap().is_none());
    assert!(decode_frame(&[88, 0, 5, 1, 2]).unwrap().is_none());
    assert!(decode_frame(&[56, 1]).unwrap().is_none());
    assert!(decode_frame(&[128, 200]).is_err());
}

/// Hand-built from the login step 157 block layout (auth preferences first),
/// not from any encoder.
#[test]
fn continue_block_reads_every_field_in_order() {
    let block = [
        0, // auth preferences flag 0: no key
        2, 1, 1, 1, 9,
        1, // staffMod, playerMod, dobVerified, quickChat, reserved, loggedInQuickChat
        0x01, 0x07, // g2 currentPlayerUid = 263
        1,    // playerIsMembers
        0xFF, 0xFF, 0xFE, // g3s lobbyDOB = -2
        1,    // loggedInMembers
        b'h', b'o', b's', b't', 0, // gjstr owner
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, // g6
    ];
    let fields = parse_continue_block(&block).unwrap();
    assert_eq!(
        fields,
        ContinueBlock {
            pid: 263,
            profile: LoginProfile {
                player_is_members: true,
                logged_in_members: true,
                player_is_quickchat: true,
                logged_in_quickchat: true,
                dob_verified: true,
                lobby_dob: -2,
                staff_mod_level: 2,
                player_mod_level: 1,
                owner: Some("host".into()),
                // Packet.g6: (g2 & 0xFFFFFFFF) << 32 + (g4s & 0xFFFFFFFF).
                server_clock: Some((0x0102_i64 << 32) + 0x0304_0506),
            },
        }
    );
    // Flag 1 carries four auth-key bytes before staffModLevel.
    let mut keyed = vec![1, 0xAA, 0xBB, 0xCC, 0xDD];
    keyed.extend_from_slice(&block[1..]);
    assert_eq!(parse_continue_block(&keyed).unwrap(), fields);
    // Any other flag value carries none (only a flag of exactly 1 does).
    let mut other = block;
    other[0] = 2;
    assert_eq!(parse_continue_block(&other).unwrap(), fields);
    // The full g6 is read; a block that ends inside it is short.
    assert!(parse_continue_block(&block[..block.len() - 1]).is_err());
    assert!(parse_continue_block(&[0, 2]).is_err());
}

#[test]
fn non_win1252_credentials_are_rejected_not_mojibake() {
    assert!(build_lobby_login_packet(&creds("😀", "pw"), 0).is_err());
    assert!(build_game_login_packet(&creds("user", "😀"), 0).is_err());
}

#[test]
fn strict_decode_names_offset_and_last_good() {
    // LOGIN phase is fail-loud: the error names the byte offset and the
    // last-good opcode (205+ are outside the dense 0-194 table).
    let state = ResyncState {
        offset: 41,
        last_good: Some(88),
    };
    let error = decode_frame_at(&[128, 200], &state).unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("200"), "{message}");
    assert!(message.contains("41"), "{message}");
    assert!(message.contains("REBUILD_NORMAL"), "{message}");
    assert!(decode_frame(&[128, 255]).is_err());
}

#[test]
fn game_decode_skips_unknown_and_continues() {
    // GAME phase is skip-and-continue: 2 opcode bytes consumed, cursor advanced,
    // history untouched.
    let mut state = ResyncState::default();
    let (first, used) = decode_frame_game(&[128, 200], &mut state).unwrap().unwrap();
    assert_eq!(
        first,
        GameDecode::Skipped {
            opcode: 200,
            offset: 0
        }
    );
    assert_eq!(used, 2);
    assert_eq!(state.offset, 2);
    assert_eq!(state.last_good, None);

    // The next byte is assumed to be an opcode (documented framing risk).
    let bytes = [128, 200, 83];
    let mut state = ResyncState::default();
    let (skipped, used) = decode_frame_game(&bytes, &mut state).unwrap().unwrap();
    assert_eq!(
        skipped,
        GameDecode::Skipped {
            opcode: 200,
            offset: 0
        }
    );
    let (second, used2) = decode_frame_game(&bytes[used..], &mut state)
        .unwrap()
        .unwrap();
    assert_eq!(
        second,
        GameDecode::Frame(Frame {
            opcode: 83,
            payload: vec![]
        })
    );
    assert_eq!(used + used2, 3);
    assert_eq!(state.last_good, Some(83));
    assert_eq!(state.offset, 3);

    // A known u16-sized frame advances the cursor by its full length and
    // records itself as last good; empty/partial input yields nothing.
    let mut state = ResyncState::default();
    let (decoded, used) = decode_frame_game(&[88, 0, 2, 9, 9], &mut state)
        .unwrap()
        .unwrap();
    assert!(matches!(decoded, GameDecode::Frame(_)));
    assert_eq!(used, 5);
    assert_eq!(
        state,
        ResyncState {
            offset: 5,
            last_good: Some(88)
        }
    );
    assert!(decode_frame_game(&[], &mut state).unwrap().is_none());
    assert!(decode_frame_game(&[88, 0], &mut state).unwrap().is_none());
}

#[test]
fn login_reply_classification_follows_the_reply_table() {
    // Step-98 reply arms.
    assert_eq!(classify_login_code(2, true), LoginCode::Proceed);
    assert_eq!(classify_login_code(2, false), LoginCode::Proceed);
    assert_eq!(classify_login_code(1, true), LoginCode::ConfirmRetry);
    assert_eq!(classify_login_code(15, false), LoginCode::AltSuccess);
    assert_eq!(classify_login_code(21, true), LoginCode::HopWait);
    assert_eq!(classify_login_code(23, false), LoginCode::WorldFull);
    assert_eq!(classify_login_code(42, true), LoginCode::Queued);
    assert_eq!(classify_login_code(49, true), LoginCode::LobbyHold);
    // 49 on a world connection fails the requestState == 132 guard
    // and falls into the fatal default arm.
    assert_eq!(classify_login_code(49, false), LoginCode::Fatal);
    // 52 is consumed on both, but only the lobby loops back.
    assert_eq!(classify_login_code(52, true), LoginCode::UrlBlock);
    assert_eq!(classify_login_code(52, false), LoginCode::UrlBlock);
    assert_eq!(classify_login_code(53, false), LoginCode::Banned);
    for code in [0u8, 3, 7, 29, 35, 45, 99, 255] {
        assert_eq!(
            classify_login_code(code, true),
            LoginCode::Fatal,
            "code {code}"
        );
        assert_eq!(
            classify_login_code(code, false),
            LoginCode::Fatal,
            "code {code}"
        );
    }
}

#[test]
fn login_clock_has_strict_cumulative_boundaries_and_pause() {
    let mut clock = LoginClock::new(true);
    assert_eq!(
        clock.budget_remaining(Duration::from_millis(500 * 20)),
        Duration::from_millis(20)
    );
    assert_eq!(
        clock.budget_remaining(Duration::from_millis(501 * 20)),
        Duration::ZERO
    );
    clock.enter_step98();
    // Step 98 raises the limit without resetting the elapsed update count.
    assert_eq!(
        clock.budget_remaining(Duration::from_millis(501 * 20)),
        Duration::from_millis(30_000)
    );
    clock.pause();
    assert!(clock.remaining().is_none());
    clock.resume();
    assert!(clock.remaining().is_some());
    // A retry attempt starts at the 2000-update threshold
    // (`loginAttempts != 0`).
    let retry = LoginClock::new(false);
    assert_eq!(
        retry.budget_remaining(Duration::from_millis(2000 * 20)),
        Duration::from_millis(20)
    );
    assert_eq!(
        retry.budget_remaining(Duration::from_millis(2001 * 20)),
        Duration::ZERO
    );
}

#[tokio::test]
async fn expired_clock_rejects_even_ready_bytes() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (mut endpoint, mut peer) = tokio::io::duplex(16);
    peer.write_all(&[2]).await.unwrap();
    let mut clock = LoginClock::new(true);
    clock.accumulated = Duration::from_millis(501 * 20);
    assert!(read_byte_clock(&mut endpoint, &clock, "expired")
        .await
        .is_err());
    let mut byte = [0];
    endpoint.read_exact(&mut byte).await.unwrap();
    assert_eq!(byte, [2], "an expired read must not consume a ready reply");
}

#[tokio::test]
async fn queued_reply_keeps_followup_reads_excluded() {
    use tokio::io::AsyncWriteExt;
    let (mut endpoint, mut peer) = tokio::io::duplex(16);
    peer.write_all(&[42, 0, 3, 2, 7]).await.unwrap();
    let mut clock = LoginClock::new(true);
    clock.enter_step98();
    let mut offset = 0;
    let mut last = None;
    let progress = LoginProgress::new();
    let result = read_login_decision(
        &mut endpoint,
        policy("world", false),
        &mut clock,
        &mut offset,
        &mut last,
        &progress,
    )
    .await
    .unwrap();
    assert!(matches!(result, Attempt::Done(Decision::Proceed)));
    assert_eq!(offset, 4);
    assert_eq!(progress.queue_position(), 3);
    assert_eq!(progress.interim_reply(), Some(42));
    assert!(
        clock.remaining().is_none(),
        "reply 2 does not clear the enter-game reply 42"
    );
    clock.accumulated = Duration::from_secs(1000);
    assert_eq!(
        read_byte_clock(&mut endpoint, &clock, "queued success block")
            .await
            .unwrap(),
        7
    );
}

#[tokio::test]
async fn transfer_disallow_reply_retains_result_and_trigger() {
    use tokio::io::AsyncWriteExt;
    let (mut endpoint, mut peer) = tokio::io::duplex(16);
    peer.write_all(&[45, 7, 0x12, 0x34]).await.unwrap();
    let mut clock = LoginClock::new(true);
    clock.enter_step98();
    let mut offset = 0;
    let mut last = None;
    let result = read_login_decision(
        &mut endpoint,
        ReplyPolicy {
            capture_disallow: true,
            ..policy("world", false)
        },
        &mut clock,
        &mut offset,
        &mut last,
        &LoginProgress::new(),
    )
    .await
    .unwrap();
    assert!(matches!(
        result,
        Attempt::Disallowed {
            reply: 45,
            result: 7,
            trigger: 0x1234
        }
    ));
    assert_eq!(offset, 4);
    assert_eq!(last, Some(45));
}

/// Login step 126: reply 21 reads one hop byte, sets `hoptime = byte * 50`
/// and the retained reply 21.
#[tokio::test]
async fn hop_wait_reply_reports_the_hop_time() {
    use tokio::io::AsyncWriteExt;
    for (hop, hoptime) in [(0u8, 0), (10, 500), (255, 12_750)] {
        let (mut endpoint, mut peer) = tokio::io::duplex(16);
        peer.write_all(&[21, hop]).await.unwrap();
        let mut clock = LoginClock::new(true);
        clock.enter_step98();
        let mut offset = 0;
        let mut last = None;
        let error = read_login_decision(
            &mut endpoint,
            policy("world", false),
            &mut clock,
            &mut offset,
            &mut last,
            &LoginProgress::new(),
        )
        .await
        .expect_err("reply 21 ends the attempt with the hop reply state");
        assert_eq!(
            error.downcast_ref::<LoginReplyState>(),
            Some(&LoginReplyState {
                reply: 21,
                hoptime,
                ban_duration: 0
            })
        );
        assert_eq!(offset, 2);
        assert_eq!(last, Some(21));
    }
}

// -- Loopback login mocks ------------------------------------------------

use tokio::net::{TcpListener, TcpStream as TestTcpStream};

/// Read one login size -2 frame; returns `(opcode, body)`.
async fn mock_read_framed_login_packet(stream: &mut TestTcpStream) -> (u8, Vec<u8>) {
    use tokio::io::AsyncReadExt;
    let mut header = [0u8; 3];
    stream.read_exact(&mut header).await.unwrap();
    let len = u16::from_be_bytes([header[1], header[2]]) as usize;
    let mut body = vec![0u8; len];
    stream.read_exact(&mut body).await.unwrap();
    (header[0], body)
}

async fn mock_handshake(stream: &mut TestTcpStream, token: i64) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut init = [0u8; 1];
    stream.read_exact(&mut init).await.unwrap();
    assert_eq!(init[0], 14);
    let mut reply = vec![0u8];
    reply.extend_from_slice(&token.to_be_bytes());
    stream.write_all(&reply).await.unwrap();
}

/// Serve one successful lobby login. The client's `LOBBYLOGIN` must be the
/// hand-written `expected` body ([`expected_login_body`]).
async fn mock_lobby_success(stream: TestTcpStream, token: i64, expected: &[u8]) {
    use tokio::io::AsyncWriteExt;
    let mut stream = stream;
    mock_handshake(&mut stream, token).await;
    let (opcode, body) = mock_read_framed_login_packet(&mut stream).await;
    assert_eq!(opcode, crate::proto::login::LOBBYLOGIN);
    assert_login_body(&body, expected, "LOBBYLOGIN");
    // [2, size:u8, payload] success reply (Lobby.ts lines 230-266).
    // Keep the post-login profile complete so the fixture exercises the
    // same lobby state fields that the retained client consumes.
    let mut profile = vec![
        0, // auth preferences marker
        0, 5, 1, // staff, player mod, DOB verified
        0, 0, 1, // lobby DOB
        0, 1, 0, // gender, quick-chat, reserved
    ];
    profile.extend_from_slice(&60_000_i64.to_be_bytes()); // membership
    profile.extend_from_slice(&[0, 0, 0, 0, 0]); // membership delay
    profile.push(3); // playerIsMembers + one more members flag
    profile.extend_from_slice(&5678_i32.to_be_bytes()); // JCoins
    profile.extend_from_slice(&1234_i32.to_be_bytes()); // loyalty
    profile.extend_from_slice(&17_u16.to_be_bytes()); // recovery day
    profile.extend_from_slice(&3_u16.to_be_bytes()); // unread messages
    profile.extend_from_slice(&42_u16.to_be_bytes()); // last login day
    profile.extend_from_slice(&0_i32.to_be_bytes()); // player host
    profile.push(1); // email status
    profile.extend_from_slice(&23_u16.to_be_bytes()); // CC expiry
    profile.extend_from_slice(&29_u16.to_be_bytes()); // grace expiry
    profile.push(1); // DOB requested
    profile.extend_from_slice(&[0, b't', b'e', b's', b't', 0]); // player name gjstr2
    profile.push(4); // members stats
    profile.extend_from_slice(&12345_i32.to_be_bytes()); // play age
    profile.extend_from_slice(&1_u16.to_be_bytes()); // target node
    profile.extend_from_slice(&[0, b'l', b'o', b'b', b'b', b'y', 0]); // target host
    profile.extend_from_slice(&43594_u16.to_be_bytes());
    profile.extend_from_slice(&43595_u16.to_be_bytes());
    let mut reply = vec![2, profile.len() as u8];
    reply.extend_from_slice(&profile);
    stream.write_all(&reply).await.unwrap();
}

/// The `CreateConnectInfo` the committed fixture is built from; the
/// server test asserts these inputs come back out of its decoder.
fn create_connect_fixture_info() -> CreateConnectInfo {
    let mut uid192 = [0u8; 24];
    for (i, byte) in uid192.iter_mut().enumerate() {
        *byte = i as u8 + 1;
    }
    CreateConnectInfo {
        uid192,
        launcher: LauncherReport {
            user_flow: [0x0102_0304, -2],
            ..LauncherReport::default()
        },
        hardware: Hardware::default(),
        crypto: LoginCrypto::Plain,
    }
}

fn create_connect_fixture_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/create_account_connect.json")
}

fn create_connect_fixture_json(packet: &[u8]) -> String {
    let info = create_connect_fixture_info();
    let doc = serde_json::json!({
        "about": "CREATE_ACCOUNT_CONNECT bytes the Rust client sends (net::create_account_connect_packet), equal to the frame written out by hand in the net tests. Decoded by server/src/lostcity/engine/LobbyAccountCreation.test.ts.",
        "regenerate": "cd tools/client910 && cargo test --lib net::tests::regenerate_create_account_connect_fixture -- --ignored",
        "inputs": {
            "uid192": info.uid192.to_vec(),
            "userFlow2": info.launcher.user_flow[0],
            "userFlow1": info.launcher.user_flow[1],
        },
        "hex": packet.iter().map(|b| format!("{b:02x}")).collect::<String>(),
    });
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

/// The `CREATE_ACCOUNT_CONNECT` frame written out by hand, with RSA and tiny
/// encryption off.
fn expected_create_account_connect(pushed_uid: [u8; 24], flow2: i32, flow1: i32) -> Vec<u8> {
    let mut body = vec![0x03, 0x8E, 0x00, 0x01]; // p2(910) p2(1)
                                                 // buildHandshakeBlock: p1(10), 4 x p4 seeds, 10 x p4, p2.
    body.push(10);
    body.extend([0u8; 16 + 40 + 2]);
    body.push(0); // pjstr(gamepack "")
    body.extend([0, 0]); // p2(playerIsAffiliate)
    body.extend(flow2.to_be_bytes()); // p4(userFlow2)
    body.extend(flow1.to_be_bytes()); // p4(userFlow1)
    body.push(0); // pjstr(unused "")
    body.push(0); // p1(language)
    body.push(0); // p1(game mode)
    body.extend(pushed_uid); // uid block
    body.push(0); // p1(additional info present ? 1 : 0)
                  // Hardware block
                  // for a zeroed platform: p1(8), 17 fixed bytes, 4 empty pjstr2
                  // (00 00 each), p1+p2 driver date, 2 empty pjstr2, p1 p1, 3 x p4,
                  // p4, 1 empty pjstr2 = 53 bytes.
    body.push(8);
    body.extend([0u8; 52]);
    body.extend([0u8; 7]); // seven bytes of tiny-encryption padding
    let mut frame = vec![crate::proto::login::CREATE_ACCOUNT_CONNECT];
    frame.extend((body.len() as u16).to_be_bytes()); // p2(0) .. psize2
    frame.extend(body);
    frame
}

#[test]
fn create_account_connect_matches_the_hand_written_block() {
    let info = create_connect_fixture_info();
    let rust = create_account_connect_packet(&info).unwrap();
    let expected = expected_create_account_connect(
        info.uid192,
        info.launcher.user_flow[0],
        info.launcher.user_flow[1],
    );
    assert_eq!(rust, expected);
    assert_eq!(rust.len(), 3 + 162);
    // An absent (all-zero) uid.dat goes out as -1s.
    let absent = create_account_connect_packet(&CreateConnectInfo::default()).unwrap();
    assert_eq!(absent, expected_create_account_connect([0xFF; 24], 0, 0));
    // The fixture the dev server decodes is these exact bytes.
    let fixture = std::fs::read_to_string(create_connect_fixture_path()).unwrap();
    assert_eq!(
        fixture,
        create_connect_fixture_json(&rust),
        "fixture is stale; regenerate it (see its `regenerate` field)"
    );
}

#[test]
#[ignore = "fixture regeneration"]
fn regenerate_create_account_connect_fixture() {
    let info = create_connect_fixture_info();
    let rust = create_account_connect_packet(&info).unwrap();
    assert_eq!(
        rust,
        expected_create_account_connect(
            info.uid192,
            info.launcher.user_flow[0],
            info.launcher.user_flow[1]
        )
    );
    std::fs::write(
        create_connect_fixture_path(),
        create_connect_fixture_json(&rust),
    )
    .unwrap();
}

#[tokio::test]
async fn account_creation_connect_opens_the_lobby_protocol_owner() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let info = create_connect_fixture_info();
    let expected = expected_create_account_connect(
        info.uid192,
        info.launcher.user_flow[0],
        info.launcher.user_flow[1],
    );
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut frame = vec![0u8; expected.len()];
        stream.read_exact(&mut frame).await.unwrap();
        assert_eq!(frame, expected);
        stream.write_all(&[2]).await.unwrap();
    });
    let (_stream, reply) = create_account_connect_at("127.0.0.1", port, &info)
        .await
        .unwrap();
    assert_eq!(reply, 2);
    server.await.unwrap();
}

/// Serve one successful world login. The client's `GAMELOGIN` must be the
/// hand-written `expected` body ([`expected_login_body`]).
async fn mock_world_success(stream: TestTcpStream, token: i64, pid: u16, expected: &[u8]) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = stream;
    mock_handshake(&mut stream, token).await;
    let (opcode, body) = mock_read_framed_login_packet(&mut stream).await;
    assert_eq!(opcode, crate::proto::login::GAMELOGIN);
    assert_login_body(&body, expected, "GAMELOGIN");
    stream.write_all(&[2]).await.unwrap();
    // Flag FIRST, then cache-typed values, repeated
    // until the final flag. A value's last byte is not a block terminator.
    stream
        .write_all(&[0, 7, 0, 0, 1, 0, 0, 0, 1, 0, 7, 1, 0, 2, 0, 0, 0, 42])
        .await
        .unwrap();
    let mut cont = [0u8; 1];
    stream.read_exact(&mut cont).await.unwrap();
    assert_eq!(cont[0], 26);
    // [2, size:u8, block]: auth flag 0,
    // six g1 fields, g2 uid, playerIsMembers, g3s lobbyDOB,
    // loggedInMembers, gjstr owner (""), g6 server clock.
    let mut block = vec![0u8, 2, 0, 0, 0, 1, 0];
    block.extend_from_slice(&pid.to_be_bytes());
    block.extend_from_slice(&[1, 0, 0, 0, 1]);
    block.push(0);
    block.extend_from_slice(&[0; 6]);
    let mut reply = vec![2, block.len() as u8];
    reply.extend_from_slice(&block);
    stream.write_all(&reply).await.unwrap();
}

#[tokio::test]
async fn lobby_login_retries_dead_connection_then_succeeds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        // Attempt 1: die right after INIT (handshake transport failure).
        let (mut s1, _) = listener.accept().await.unwrap();
        {
            use tokio::io::AsyncReadExt;
            let mut byte = [0u8; 1];
            s1.read_exact(&mut byte).await.unwrap();
        }
        drop(s1);
        // Attempt 2: full success.
        let (s2, _) = listener.accept().await.unwrap();
        let expected = expected_login_body(&LoginCase::fresh(Kind::Lobby, 0x0102030405060708));
        mock_lobby_success(s2, 0x0102030405060708, &expected).await;
    });
    let (_stream, ok) = login_lobby(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap();
    assert_eq!(ok.pid, None);
    assert_eq!(ok.server_token, 0x0102030405060708);
    assert!(ok.profile.player_is_members, "{ok:?}");
    assert!(ok.lobby.membership_flag);
    assert_eq!(ok.profile.player_mod_level, 5);
    assert_eq!(ok.lobby.unread_messages, 3);
    assert_eq!(ok.lobby.recovery_day, 17);
    assert_eq!(ok.lobby.jcoins_balance, 5678);
    assert_eq!(ok.lobby.loyalty_balance, 1234);
    assert_eq!(ok.lobby.last_login_day, 42);
    assert_eq!(ok.lobby.email_status, 1);
    assert_eq!(ok.lobby.cc_expiry, 23);
    assert_eq!(ok.lobby.grace_expiry, 29);
    assert!(ok.lobby.dob_requested);
    assert_eq!(ok.lobby.members_stats, 4);
    assert_eq!(ok.lobby.play_age, 12345);
    assert_eq!(ok.lobby.player_name, "test");
    assert_eq!(ok.lobby.world_id, Some(1));
    assert_eq!(ok.lobby.world_host, "lobby");
    assert_eq!(ok.lobby.world_port, 43594);
    assert_eq!(ok.lobby.world_port2, 43595);
    server.await.unwrap();
}

#[tokio::test]
async fn lobby_login_fails_fast_on_handshake_reject() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let (mut s, _) = listener.accept().await.unwrap();
        let mut init = [0u8; 1];
        s.read_exact(&mut init).await.unwrap();
        // Non-zero handshake code ends login immediately, no retry (step 35).
        let mut reply = vec![11u8];
        reply.extend_from_slice(&0i64.to_be_bytes());
        s.write_all(&reply).await.unwrap();
        listener
    });
    let error = login_lobby(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap_err();
    // Step 35 ends the login (`loginStep = 7`), it is not an exhausted
    // retry loop and no login reply block was read.
    assert!(
        error.downcast_ref::<LoginAttemptsExhausted>().is_none(),
        "{error:#}"
    );
    // The rejection code is the login's reply, for the login screens.
    assert_eq!(
        error
            .downcast_ref::<LoginReplyState>()
            .map(|state| state.reply),
        Some(11),
        "{error:#}"
    );
    // No second connection: a retry would already sit in the backlog, since
    // `login_lobby` only returns after its last attempt.
    let listener = server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::ZERO, listener.accept())
            .await
            .is_err(),
        "a rejected handshake must not reconnect"
    );
}

#[tokio::test]
async fn lobby_login_fails_fast_on_fatal_code() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        let (s, _) = listener.accept().await.unwrap();
        let mut s = s;
        mock_handshake(&mut s, 0).await;
        let _ = mock_read_framed_login_packet(&mut s).await;
        // Fatal reply (default arm).
        s.write_all(&[3]).await.unwrap();
    });
    let error = login_lobby(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap_err();
    // `setReply(3)` ends the login on the first attempt (no retry loop).
    assert_eq!(
        error.downcast_ref::<LoginReplyState>(),
        Some(&LoginReplyState {
            reply: 3,
            hoptime: 0,
            ban_duration: 0
        }),
        "{error:#}"
    );
    server.await.unwrap();
}

#[tokio::test]
async fn lobby_login_reconnects_on_world_full_then_succeeds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        // Attempt 1: reply 23 -> immediate reconnect.
        let (s1, _) = listener.accept().await.unwrap();
        let mut s1 = s1;
        mock_handshake(&mut s1, 0).await;
        let _ = mock_read_framed_login_packet(&mut s1).await;
        s1.write_all(&[23]).await.unwrap();
        // Attempt 2: full success.
        let (s2, _) = listener.accept().await.unwrap();
        mock_lobby_success(
            s2,
            7,
            &expected_login_body(&LoginCase::fresh(Kind::Lobby, 7)),
        )
        .await;
    });
    let (_stream, ok) = login_lobby(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap();
    assert_eq!(ok.server_token, 7);
    server.await.unwrap();
}

#[tokio::test]
async fn world_login_succeeds_end_to_end() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (s, _) = listener.accept().await.unwrap();
        let expected = expected_login_body(&LoginCase::fresh(Kind::World, 0x1112131415161718));
        mock_world_success(s, 0x1112131415161718, 7, &expected).await;
    });
    let (_stream, ok) = login_world(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap();
    assert_eq!(ok.pid, Some(7));
    assert_eq!(ok.server_token, 0x1112131415161718);
    assert_eq!(ok.server_varcs, vec![0, 1, 0, 0, 0, 1, 0, 2, 0, 0, 0, 42]);
    server.await.unwrap();
}

#[tokio::test]
async fn world_login_reconnects_on_queue_then_succeeds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        use tokio::io::AsyncWriteExt;
        // Attempt 1: reply 42 (queue pos 5), then drop mid-decision. The
        // client consumes the follow-up u16 on the SAME connection
        // (step 215) and
        // reconnects when the next reply read hits EOF.
        let (s1, _) = listener.accept().await.unwrap();
        let mut s1 = s1;
        mock_handshake(&mut s1, 0).await;
        let _ = mock_read_framed_login_packet(&mut s1).await;
        s1.write_all(&[42, 0, 5]).await.unwrap();
        drop(s1);
        // Attempt 2: full success after the reconnect.
        let (s2, _) = listener.accept().await.unwrap();
        mock_world_success(
            s2,
            9,
            11,
            &expected_login_body(&LoginCase::fresh(Kind::World, 9)),
        )
        .await;
    });
    let (_stream, ok) = login_world(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .unwrap();
    assert_eq!(ok.pid, Some(11));
    server.await.unwrap();
}

// -- LOBBYLOGIN / GAMELOGIN bodies written out by hand ---------------------

/// Which login step 84 builds (lobby or game).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Lobby,
    World,
}

/// The TOTP detail branches this client can take. `AUTH_FOUND` needs a stored
/// auth-preference entry, which this client never holds.
#[derive(Clone, Copy, Debug)]
enum TotpCase {
    NotFound,
    /// `newAuthPreference` (six digits) with `authDontTrust == false`.
    Trust(u32),
    /// `newAuthPreference` (six digits) with `authDontTrust == true`.
    DontTrust(u32),
}

/// The inputs of one reference login. The other client fields the bodies read
/// are fixed at the values this client stands in for them (see
/// [`expected_login_body`]).
#[derive(Clone, Debug)]
struct LoginCase {
    kind: Kind,
    username: &'static str,
    password: &'static str,
    server_token: i64,
    site_settings: &'static str,
    /// uid.dat as read; all zero = absent.
    uid192: [u8; 24],
    totp: TotpCase,
    /// A reconnect (client state 14).
    reconnect: bool,
}

impl LoginCase {
    /// "test"/"secret", no site settings, no uid.dat, no auth preference.
    fn fresh(kind: Kind, server_token: i64) -> Self {
        Self {
            kind,
            username: "test",
            password: "secret",
            server_token,
            site_settings: "",
            uid192: [0; 24],
            totp: TotpCase::NotFound,
            reconnect: false,
        }
    }

    /// The same inputs as the client's [`LoginParams`].
    fn params(&self, port: u16) -> LoginParams {
        let (new_auth_preference, auth_dont_trust) = match self.totp {
            TotpCase::NotFound => (String::new(), false),
            TotpCase::Trust(pin) => (format!("{pin:06}"), false),
            TotpCase::DontTrust(pin) => (format!("{pin:06}"), true),
        };
        LoginParams {
            port,
            ..params(
                self.username,
                self.password,
                self.site_settings,
                &self.uid192,
                &AuthOptions {
                    new_auth_preference,
                    auth_dont_trust,
                    reconnect: self.reconnect,
                },
            )
        }
    }
}

/// `Packet.pjstr` of a Latin-1 string (cp1252 equals Latin-1 on 0xA0-0xFF,
/// the only non-ASCII range these vectors use).
fn write_pjstr(out: &mut Vec<u8>, value: &str) {
    out.extend(value.chars().map(|c| u8::try_from(u32::from(c)).unwrap()));
    out.push(0);
}

/// The RSA stub block with RSA off, so `rsaenc` leaves it as is.
fn expected_rsa_block(login: &LoginCase) -> Vec<u8> {
    // RSA stub: p1(10), the four outKey seeds (the client
    // sends zeros: ISAAC is off on both ends), p8(serverToken), and outKey2
    // on a reconnect.
    let mut out = vec![10];
    out.extend([0u8; 16]);
    out.extend(login.server_token.to_be_bytes());
    if login.reconnect {
        // A reconnect sends the previous seeds and no credentials.
        out.extend([0u8; 16]);
        return out;
    }
    // The authenticator block starts with the type's wire id: stored token 0,
    // don't-trust 1, none 2, trust 3.
    match login.totp {
        TotpCase::NotFound => {
            out.push(2);
            out.extend([0u8; 4]);
        }
        TotpCase::Trust(pin) | TotpCase::DontTrust(pin) => {
            out.push(if matches!(login.totp, TotpCase::Trust(_)) {
                3
            } else {
                1
            });
            out.extend(&pin.to_be_bytes()[1..]); // p3(parseInt(pref))
            out.push(0);
        }
    }
    out.push(0); // pbool(false)
    write_pjstr(&mut out, login.password);
    out.extend([0u8; 8]); // p8(socialname)
    out.extend([0u8; 8]); // p8(unused 64-bit login field)
    out
}

/// The body of login step 84 (GAMELOGIN and LOBBYLOGIN) for a non-social
/// login with tiny encryption off, written out by hand. The values the client
/// stands in: game mode 0, language 0, window mode 0, canvas 800x600,
/// antialiasing 0, affiliate 0, verify id 0, userFlow2/1 0, two unused flags
/// 0, an unused empty string, no additional info, javascriptEnabled and
/// haveChrome false, clientType 0, an unused int 0, gamepack "", no world-hop
/// target, current lobby node 1, no JS5 archives loaded (checksum 0), a zeroed
/// hardware block and a 58-byte zeroed stand-in for the preferences block.
fn expected_login_body(login: &LoginCase) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(910i32.to_be_bytes()); // p4(910)
    out.extend(1i32.to_be_bytes()); // p4(1)
    if login.kind == Kind::World {
        out.push(u8::from(login.reconnect)); // p1(reconnect ? 1 : 0)
    }
    out.extend(expected_rsa_block(login));
    out.push(1); // p1(socialKey == -1 ? 1 : 0), then pjstr(username)
    write_pjstr(&mut out, login.username);
    if login.kind == Kind::Lobby {
        out.extend([0, 0]); // p1(game mode) p1(language id)
    }
    out.push(0); // p1(window mode)
    out.extend(800u16.to_be_bytes()); // p2(canvasWid)
    out.extend(600u16.to_be_bytes()); // p2(canvasHei)
    out.push(0); // p1(antialiasing)
                 // Uid block: all zero -> 24 x -1.
    if login.uid192 == [0; 24] {
        out.extend([0xFF; 24]);
    } else {
        out.extend(login.uid192);
    }
    write_pjstr(&mut out, login.site_settings);
    if login.kind == Kind::World {
        out.extend([0u8; 4]); // p4(playerIsAffiliate)
    }
    out.push(58); // p1(prefs.pos)
    out.extend([0u8; 58]); // prefs block stand-in
                           // Hardware block
                           // of a zeroed platform: p1(8), 17 fixed bytes interleaved with seven
                           // empty pjstr2 (00 00 each) = 53 bytes.
    out.push(8);
    out.extend([0u8; 52]);
    match login.kind {
        Kind::World => {
            // p4 verifyId, userFlow2, userFlow1, two unused flags
            out.extend([0u8; 20]);
            write_pjstr(&mut out, ""); // unused string
            out.push(0); // no additional info
            out.extend([0, 0, 0]); // javascriptEnabled, haveChrome, clientType & 1
            out.extend([0u8; 4]); // p4(unused int)
            write_pjstr(&mut out, ""); // gamepack
            out.push(1); // no world-hop target
            out.extend(1u16.to_be_bytes()); // p2(current lobby node)
        }
        Kind::Lobby => {
            out.extend([0u8; 4]); // p4(verify id)
            write_pjstr(&mut out, ""); // unused string
            out.extend([0u8; 4]); // p4(playerIsAffiliate)
            out.extend([0u8; 4]); // p4(unused int)
            write_pjstr(&mut out, ""); // gamepack
            out.push(0); // p1(clientType & 1)
            out.push(0); // pbool(false)
        }
    }
    // pushJS5CRCs: every Js5Archive but LOADING_SPRITES (41).
    out.extend([0u8; 41 * 4]);
    out
}

/// Byte equality with the first differing offset in the message.
fn assert_login_body(actual: &[u8], expected: &[u8], what: &str) {
    let first = actual.iter().zip(expected).position(|(a, e)| a != e);
    assert!(
        actual == expected,
        "{what}: first difference at {first:?}, lengths {} vs {}",
        actual.len(),
        expected.len()
    );
}

/// `login_lobby`/`login_world` over a loopback socket send, byte for byte,
/// the LOBBYLOGIN/GAMELOGIN body the reference layout gives for the same inputs:
/// both TOTP preference branches and none, reconnect, uid.dat present and
/// absent, site settings, a Latin-1 password and the GAMELOGIN tail.
#[tokio::test]
async fn login_workers_send_the_expected_login_bodies() {
    let uid: [u8; 24] = std::array::from_fn(|index| index as u8 + 1);
    let token = 0x0A0B_0C0D_0E0F_1011;
    let base = |kind| LoginCase {
        username: "user",
        password: "p\u{e9}ss",
        site_settings: "site=foo",
        uid192: uid,
        ..LoginCase::fresh(kind, token)
    };
    let mut cases = Vec::new();
    for kind in [Kind::Lobby, Kind::World] {
        cases.push(base(kind));
        cases.push(LoginCase {
            uid192: [0; 24],
            site_settings: "",
            ..base(kind)
        });
        cases.push(LoginCase {
            totp: TotpCase::Trust(123_456),
            ..base(kind)
        });
        cases.push(LoginCase {
            totp: TotpCase::DontTrust(654_321),
            ..base(kind)
        });
    }
    cases.push(LoginCase {
        reconnect: true,
        ..base(Kind::World)
    });
    cases.push(LoginCase {
        reconnect: true,
        totp: TotpCase::DontTrust(12),
        ..base(Kind::World)
    });
    for case in cases {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let expected = expected_login_body(&case);
        let kind = case.kind;
        let server = tokio::spawn(async move {
            let (s, _) = listener.accept().await.unwrap();
            match kind {
                Kind::Lobby => mock_lobby_success(s, token, &expected).await,
                Kind::World => mock_world_success(s, token, 3, &expected).await,
            }
        });
        let p = case.params(port);
        let result = match case.kind {
            Kind::Lobby => login_lobby(&p).await,
            Kind::World => login_world(&p).await,
        };
        result.unwrap_or_else(|error| panic!("{case:?}: {error:#}"));
        server
            .await
            .unwrap_or_else(|error| panic!("{case:?}: {error}"));
    }
}

mod network_packets;
mod session_tests;

#[cfg(test)]
mod secure_login_tests;
