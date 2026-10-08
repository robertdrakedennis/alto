use super::*;
use native910::{
    script::Operand,
    vm::{Host, InstructionContext},
};

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn object_and_loc_params_use_the_recorded_id_order_and_defaults() {
    let operand = Operand::Byte(0);
    let mut engine = Engine::default();
    let pack = crate::test_support::require_pack("client.obj.config.js5");
    engine.load_cache(&pack);
    engine.configs.params.insert(
        43,
        native910::config::ParamConfig {
            kind: Some(0),
            default_int: Some(9),
            ..Default::default()
        },
    );
    for command in ["oc_param", "lc_param", "seq_param"] {
        let context = InstructionContext {
            script_name: Some("type-param-test"),
            script_id: None,
            event: None,
            pc: 0,
            command,
            operand: &operand,
            secondary: false,
            int_locals: &[],
        };
        let mut ints = vec![123, 43];
        let value = engine
            .trap_context(&context, &mut ints, &mut Vec::new(), &mut Vec::new())
            .unwrap();
        assert_eq!(value, Some(Value::Int(9)), "{command}");
        assert!(ints.is_empty(), "{command}");
    }
}

/// Runs one account-creation CS2 command through the production trap
/// (`Engine::trap_context`) and returns the bytes it queued on the lobby
/// writer.
fn account_creation_frame(
    engine: &mut Engine,
    command: &'static str,
    mut ints: Vec<i32>,
    objs: &[&str],
) -> Vec<u8> {
    let operand = Operand::Byte(0);
    let context = InstructionContext {
        script_name: Some("account-creation-request-test"),
        script_id: None,
        event: None,
        pc: 0,
        command,
        operand: &operand,
        secondary: false,
        int_locals: &[],
    };
    let mut objs: Vec<String> = objs.iter().map(|s| (*s).to_owned()).collect();
    engine.outgoing.clear();
    engine
        .trap_context(&context, &mut ints, &mut objs, &mut Vec::new())
        .unwrap();
    assert!(
        ints.is_empty() && objs.is_empty(),
        "{command} must pop its operands"
    );
    std::mem::take(&mut engine.outgoing)
}

/// The CS2 inputs of the account-creation frames, in the original client stack order.
/// Shared with the committed fixture that the dev server decodes.
const ACCOUNT_CREATION_INPUTS: &[(&str, &[i32], &[&str])] = &[
    ("create_availablerequest", &[], &["a@b"]),
    ("create_name_availablerequest", &[], &["Run\u{e9}"]),
    ("create_suggest_name_request", &[], &[]),
    ("create_createrequest", &[12, 1], &["user", "pw", "a@b"]),
    ("create_step_reached", &[7], &[]),
];

fn account_creation_fixture_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/account_creation_lobby_frames.json")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Expected wire bytes hand-derived from the original source. With
/// With ISAAC disabled the next value is 0, so the opcode is written bare; with
/// tinyenc disabled the encoder is a no-op, leaving the seven reserved bytes
/// zero. `queue` /
/// `flush` write `buf.data[0..pos]` verbatim.
fn expected_account_creation_frames() -> Vec<Vec<u8>> {
    vec![
        // the email availability request for "a@b":
        //   pIsaac1(CREATE_CHECK_EMAIL=28)      -> 1c
        //   p2(0)  [size placeholder]           -> 00 00, start = 3
        //   pjstr("a@b")                        -> 61 40 62 00      (pos 7)
        //   pos += 7                            -> 00 x7            (pos 14)
        //   psize2(14 - 3 = 11) backfills data[1..3] = 00 0b
        vec![28, 0x00, 0x0b, b'a', b'@', b'b', 0, 0, 0, 0, 0, 0, 0, 0],
        // the display-name availability request for "Run\u{e9}":
        //   pIsaac1(CREATE_CHECK_NAME=65)       -> 41
        //   p1(0)  [size placeholder]           -> 00, start = 2
        //   pjstr: Cp1252 'R' 'u' 'n' 0xE9, NUL -> 52 75 6e e9 00  (pos 7)
        //   pos += 7                                                (pos 14)
        //   psize1(14 - 2 = 12) backfills data[1] = 0c
        vec![65, 0x0c, b'R', b'u', b'n', 0xe9, 0, 0, 0, 0, 0, 0, 0, 0],
        // requestDisplayNameSuggestion: CREATE_SUGGEST_NAMES
        // has size 0 -> opcode only.
        vec![69],
        // create_createrequest pops three
        // strings [user, pw, a@b] and ints [12, 1] and calls
        // requestAccountCreation(user, pw, 12, true, a@b):
        //   pIsaac1(CREATE_ACCOUNT=20)          -> 14
        //   p2(0)  [size placeholder]           -> 00 00, start = 3
        //   pjstr("user")                       -> 75 73 65 72 00  (pos 8)
        //   pjstr("pw")                         -> 70 77 00        (pos 11)
        //   p1(12) p1(1)                        -> 0c 01           (pos 13)
        //   pjstr("a@b")                        -> 61 40 62 00     (pos 17)
        //   pos += 7                                               (pos 24)
        //   psize2(24 - 3 = 21) backfills data[1..3] = 00 15
        [
            vec![20, 0x00, 0x15],
            b"user\0pw\0".to_vec(),
            vec![12, 1],
            b"a@b\0".to_vec(),
            vec![0; 7],
        ]
        .concat(),
        // requestStatsLogging(7): CREATE_LOG_PROGRESS has
        // fixed size 1 -> opcode + p1(7).
        vec![118, 7],
    ]
}

fn rust_account_creation_frames() -> Vec<Vec<u8>> {
    // state == 0 after a SUCCESSFUL connect reply.
    let mut engine = {
        let mut engine = Engine::default();
        engine.login.account_creation_connected = true;
        engine
    };
    ACCOUNT_CREATION_INPUTS
        .iter()
        .map(|(command, ints, objs)| {
            account_creation_frame(&mut engine, command, ints.to_vec(), objs)
        })
        .collect()
}

fn account_creation_fixture_json(frames: &[Vec<u8>]) -> String {
    let entries: Vec<serde_json::Value> = ACCOUNT_CREATION_INPUTS
        .iter()
        .zip(frames)
        .map(|((command, ints, objs), frame)| {
            serde_json::json!({
                "command": command,
                "ints": ints,
                "objs": objs,
                "hex": hex(frame),
            })
        })
        .collect();
    let doc = serde_json::json!({
        "about": "Lobby-stream bytes the Rust client queues for the CS2 account-creation commands. Equal to the frames derived by hand from the account-creation message layout (see no_op_host_tests.rs expected_account_creation_frames). Decoded by server/src/lostcity/engine/LobbyAccountCreation.test.ts.",
        "regenerate": "cd tools/client910 && cargo test --lib ui_runtime::no_op_host_tests::regenerate_account_creation_lobby_frames_fixture -- --ignored",
        "frames": entries,
    });
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

#[test]
fn account_creation_requests_use_the_expected_lobby_frames() {
    let expected = expected_account_creation_frames();
    let rust = rust_account_creation_frames();
    for (((command, _, _), expected), rust) in
        ACCOUNT_CREATION_INPUTS.iter().zip(&expected).zip(&rust)
    {
        assert_eq!(
            hex(rust),
            hex(expected),
            "{command} frame differs from the expected bytes"
        );
    }
    // The committed fixture the server decodes must be these exact bytes.
    let fixture = std::fs::read_to_string(account_creation_fixture_path()).unwrap();
    assert_eq!(
        fixture,
        account_creation_fixture_json(&rust),
        "fixture is stale; regenerate it (see its `regenerate` field)"
    );

    // Reply-owner side effects of the same original methods.
    let mut engine = {
        let mut engine = Engine::default();
        engine.login.account_creation_connected = true;
        engine
    };
    account_creation_frame(&mut engine, "create_availablerequest", vec![], &["a@b"]);
    assert_eq!(engine.creation.email_reply, -3);
    account_creation_frame(&mut engine, "create_name_availablerequest", vec![], &["n"]);
    assert_eq!(engine.creation.name_reply, -3);
    account_creation_frame(&mut engine, "create_suggest_name_request", vec![], &[]);
    assert_eq!(engine.creation.suggest_reply, -3);
    assert_eq!(engine.creation.suggested_name, None);
    assert!(!engine.creation.is_under13);
    account_creation_frame(
        &mut engine,
        "create_createrequest",
        vec![12, 1],
        &["u", "p", "e"],
    );
    assert_eq!(engine.creation.account_reply, -3);
    assert!(
        engine.creation.is_under13,
        "requestAccountCreation:137-139 age < 13"
    );

    let operand = Operand::Byte(0);
    let context = InstructionContext {
        script_name: Some("account-creation-request-test"),
        script_id: None,
        event: None,
        pc: 0,
        command: "create_connectrequest",
        operand: &operand,
        secondary: false,
        int_locals: &[],
    };
    let mut engine = Engine::default();
    let mut longs = Vec::new();

    // The connect request is gated on title state 4 (login ready), not the
    // lobby.
    engine.login.lobby_login = true;
    engine.login.ready = false;
    let connect_context = InstructionContext {
        command: "create_connectrequest",
        ..context
    };
    engine
        .trap_context(
            &connect_context,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut longs,
        )
        .unwrap();
    assert!(!engine.creation.connect_requested);
    engine.login.lobby_login = false;
    engine.login.ready = true;
    let connect_context = InstructionContext {
        command: "create_connectrequest",
        ..context
    };
    engine
        .trap_context(
            &connect_context,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut longs,
        )
        .unwrap();
    assert!(engine.creation.connect_requested);
    assert!(engine.creation.connect_in_progress);
    let connect_reply = InstructionContext {
        command: "create_connect_reply",
        ..context
    };
    assert_eq!(
        engine
            .trap_context(&connect_reply, &mut Vec::new(), &mut Vec::new(), &mut longs,)
            .unwrap(),
        Some(Value::Int(-2))
    );

    engine.outgoing.clear();
    let under13_context = InstructionContext {
        command: "create_under13",
        ..context
    };
    assert_eq!(
        engine
            .trap_context(
                &under13_context,
                &mut Vec::new(),
                &mut Vec::new(),
                &mut longs,
            )
            .unwrap(),
        Some(Value::Int(0))
    );
    let set_under13_context = InstructionContext {
        command: "create_setunder13",
        ..context
    };
    engine
        .trap_context(
            &set_under13_context,
            &mut Vec::new(),
            &mut Vec::new(),
            &mut longs,
        )
        .unwrap();
    assert!(engine.creation.is_under13);
}

/// Every create_* request returns
/// before queueing (and before its reply/under-13 side effects) unless
/// `state == 0`. The operands have already been popped.
#[test]
fn account_creation_requests_need_state_zero() {
    for connected_state in [false, true] {
        // Title/lobby states where the ordinary login owners are live.
        let mut engine = {
            let mut engine = Engine::default();
            engine.login.account_creation_connected = connected_state;
            engine.login.ready = true;
            engine.login.lobby_login = !connected_state;
            engine
        };
        let frames: Vec<Vec<u8>> = ACCOUNT_CREATION_INPUTS
            .iter()
            .map(|(command, ints, objs)| {
                account_creation_frame(&mut engine, command, ints.to_vec(), objs)
            })
            .collect();
        if connected_state {
            assert_eq!(frames, expected_account_creation_frames());
            assert_eq!(engine.creation.email_reply, -3);
            assert_eq!(engine.creation.account_reply, -3);
            assert!(engine.creation.is_under13);
        } else {
            assert!(frames.iter().all(Vec::is_empty), "{frames:02x?}");
            let fresh = Engine::default();
            assert_eq!(engine.creation.email_reply, fresh.creation.email_reply);
            assert_eq!(engine.creation.name_reply, fresh.creation.name_reply);
            assert_eq!(engine.creation.suggest_reply, fresh.creation.suggest_reply);
            assert_eq!(engine.creation.account_reply, fresh.creation.account_reply);
            assert!(
                !engine.creation.is_under13,
                "age 12 outside state 0 must not flag under-13"
            );
        }
    }
}

/// Rewrites the committed fixture from the production encoder. Run it
/// only after `account_creation_requests_use_the_expected_lobby_frames` proves
/// the encoder matches the hand-derived bytes.
#[test]
#[ignore = "fixture regeneration"]
fn regenerate_account_creation_lobby_frames_fixture() {
    let rust = rust_account_creation_frames();
    assert_eq!(rust, expected_account_creation_frames());
    std::fs::write(
        account_creation_fixture_path(),
        account_creation_fixture_json(&rust),
    )
    .unwrap();
}
