//! The login flows the ordinary login tests leave out: the social network
//! sign-on, an in-place reconnect (reply 15), a login parked on reply 1 and
//! the device check (replies 49 and 52). Each runs the real workers over a
//! loopback or in-memory connection against a server written from the
//! original client's behaviour.
use super::*;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// The server token of the recording.
const TOKEN: i64 = 0x0A0B_0C0D_0E0F_1011;
/// The key and name word the recorded server gave.
const SOCIAL_KEY: i64 = 0x1122_3344_5566_7788;
const SOCIAL_NAME: i64 = 0x0102_0304_0506_0708;
/// The page the scripted servers name for the player to sign in on.
const SIGN_ON_PAGE: &str = "http://localhost/signon";

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
        .collect()
}

/// The `name hex` lines of a recording.
fn recording(file: &str) -> Vec<(String, Vec<u8>)> {
    rs910_core::test_support::frozen::text(file)
        .lines()
        .map(|line| {
            let (name, hex) = line.split_once(' ').unwrap();
            (name.to_owned(), unhex(hex))
        })
        .collect()
}

fn sso_params(port: u16, social_key: Option<i64>) -> LoginParams {
    LoginParams {
        sso: Some(SsoLogin {
            network: 6,
            social_key,
            social_name: SOCIAL_NAME,
        }),
        ..LoginParams::new("127.0.0.1", port, "", "")
    }
}

/// What the original client wrote for a social sign-on (zero seeds, no
/// encryption): the login's RSA block for a known key, and the packet that
/// opens a sign-on, equal what the builders write.
#[test]
fn social_login_matches_the_recording() {
    // The account the recorded server gave.
    let account = rs910_core::test_support::frozen::text("social-login/account.txt");
    let mut lines = account.lines();
    let key = lines.next().unwrap().strip_prefix("key ").unwrap();
    let name = lines.next().unwrap().strip_prefix("name ").unwrap();
    assert_eq!(i64::from_str_radix(key, 16).unwrap(), SOCIAL_KEY);
    assert_eq!(i64::from_str_radix(name, 16).unwrap(), SOCIAL_NAME);

    let mut seen = Vec::new();
    for (case, block) in recording("social-login/rsa-blocks.txt") {
        let (new_auth_preference, auth_dont_trust) = match case.as_str() {
            "sso-code" => ("123456", true),
            _ => ("", false),
        };
        let reconnect = case == "sso-reconnect";
        let login = LoginParams {
            auth: AuthOptions {
                new_auth_preference: new_auth_preference.into(),
                auth_dont_trust,
                reconnect,
            },
            ..sso_params(0, Some(SOCIAL_KEY))
        };
        // The identity that follows the block is the social key, not a name.
        let mut identity = vec![0];
        identity.extend(SOCIAL_KEY.to_be_bytes());
        let game = game_login_payload(&login, TOKEN).unwrap();
        assert_eq!(&game[9..9 + block.len()], &block[..], "{case}: game login");
        assert_eq!(
            &game[9 + block.len()..9 + block.len() + identity.len()],
            &identity[..],
            "{case}: game login identity"
        );
        if !reconnect {
            let lobby = lobby_login_payload(&login, TOKEN).unwrap();
            assert_eq!(
                &lobby[8..8 + block.len()],
                &block[..],
                "{case}: lobby login"
            );
            assert_eq!(
                &lobby[8 + block.len()..8 + block.len() + identity.len()],
                &identity[..],
                "{case}: lobby login identity"
            );
        }
        seen.push(case);
    }
    assert_eq!(seen, ["sso", "sso-code", "sso-reconnect"]);

    let mut seen = Vec::new();
    for (case, packet) in recording("social-login/connect.txt") {
        let (kind, reconnect) = match case.as_str() {
            "world" => (ConnectionKind::World, false),
            "world-reconnect" => (ConnectionKind::World, true),
            "lobby" => (ConnectionKind::Lobby, false),
            other => panic!("unknown recorded connection {other}"),
        };
        let login = LoginParams {
            auth: AuthOptions {
                reconnect,
                ..AuthOptions::default()
            },
            ..sso_params(0, None)
        };
        assert_eq!(
            build_social_connection_packet(kind, &login, TOKEN).unwrap(),
            packet,
            "{case}: INIT_SOCIAL_NETWORK_CONNECTION"
        );
        seen.push(case);
    }
    assert_eq!(seen, ["world", "world-reconnect", "lobby"]);
}

/// The server's half of a social sign-on after its handshake: read the
/// opening packet (which must be the recorded one), name the page, tell the
/// client the player has signed in (after a stray byte it must ignore) and
/// give the account.
async fn serve_sign_on(stream: &mut TcpStream_, expected_connect: &[u8]) {
    let (opcode, body) = mock_read_framed_login_packet(stream).await;
    assert_eq!(opcode, crate::proto::login::INIT_SOCIAL_NETWORK_CONNECTION);
    assert_eq!(body, expected_connect, "the sign-on's opening packet");
    // The page: a `gjstr2` string (version byte, text, terminator).
    let mut page = vec![0];
    page.extend_from_slice(SIGN_ON_PAGE.as_bytes());
    page.push(0);
    let mut reply = (page.len() as u16).to_be_bytes().to_vec();
    reply.extend(page);
    reply.extend([0, 0, 1]); // stray bytes, then the confirmation
    reply.extend(SOCIAL_KEY.to_be_bytes());
    reply.extend(SOCIAL_NAME.to_be_bytes());
    stream.write_all(&reply).await.unwrap();
}

type TcpStream_ = tokio::net::TcpStream;

/// What follows an accepted world login: the var block, `GAMELOGIN_CONTINUE`
/// and the continuation block.
async fn serve_world_session(stream: &mut TcpStream_, pid: u16) {
    stream.write_all(&[2]).await.unwrap();
    stream.write_all(&[0, 1, 1]).await.unwrap();
    let mut cont = [0u8; 1];
    stream.read_exact(&mut cont).await.unwrap();
    assert_eq!(cont[0], 26);
    let mut block = vec![0u8, 2, 0, 0, 0, 1, 0];
    block.extend_from_slice(&pid.to_be_bytes());
    block.extend_from_slice(&[1, 0, 0, 0, 1, 0]);
    block.extend_from_slice(&[0; 6]);
    let mut reply = vec![2, block.len() as u8];
    reply.extend_from_slice(&block);
    stream.write_all(&reply).await.unwrap();
}

fn recorded_connect(case: &str) -> Vec<u8> {
    recording("social-login/connect.txt")
        .into_iter()
        .find(|(name, _)| name == case)
        .unwrap()
        .1[3..]
        .to_vec()
}

/// A world login through a social network with no key yet: the client opens
/// the sign-on (the recorded packet), hands the page to the login screen,
/// waits out the player, reads the account, logs in with the social login
/// block (no header, RSA block or identity) and remembers the key and name.
#[tokio::test]
async fn social_sign_on_negotiates_a_key_then_logs_in() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let params = sso_params(port, None);
    let expected_login = social_login_payload(ConnectionKind::World, &params).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        mock_handshake(&mut stream, TOKEN).await;
        serve_sign_on(&mut stream, &recorded_connect("world")).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut stream).await;
        assert_eq!(opcode, crate::proto::login::SOCIAL_NETWORK_LOGIN);
        assert_eq!(body, expected_login, "the social login block");
        serve_world_session(&mut stream, 12).await;
    });
    let (_stream, ok) = login_world(&params).await.unwrap();
    server.await.unwrap();
    assert_eq!(ok.pid, Some(12));
    assert_eq!(params.progress.social(), Some((SOCIAL_KEY, SOCIAL_NAME)));
    assert_eq!(params.progress.take_urls(), [SIGN_ON_PAGE]);
}

/// The lobby signs on the same way, and a login that presents a key the
/// server does not know (reply 35) signs on again on a new connection.
#[tokio::test]
async fn an_unknown_social_key_starts_the_sign_on_over() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let params = sso_params(port, Some(SOCIAL_KEY ^ 1));
    let known = game_login_payload(&params, TOKEN).unwrap();
    let renegotiated = social_login_payload(ConnectionKind::World, &params).unwrap();
    let server = tokio::spawn(async move {
        // The stale key is presented and refused.
        let (mut first, _) = listener.accept().await.unwrap();
        mock_handshake(&mut first, TOKEN).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut first).await;
        assert_eq!(opcode, crate::proto::login::GAMELOGIN);
        assert_eq!(body, known, "the login presents the key it holds");
        first.write_all(&[35]).await.unwrap();
        drop(first);
        // The sign-on starts over.
        let (mut second, _) = listener.accept().await.unwrap();
        mock_handshake(&mut second, TOKEN).await;
        serve_sign_on(&mut second, &recorded_connect("world")).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut second).await;
        assert_eq!(opcode, crate::proto::login::SOCIAL_NETWORK_LOGIN);
        assert_eq!(body, renegotiated);
        serve_world_session(&mut second, 3).await;
    });
    let (_stream, ok) = login_world(&params).await.unwrap();
    server.await.unwrap();
    assert_eq!(ok.pid, Some(3));
    assert_eq!(params.progress.social(), Some((SOCIAL_KEY, SOCIAL_NAME)));
}

/// Reply 35 to a login that has just signed on is final, not another sign-on.
#[tokio::test]
async fn a_refused_new_social_key_is_final() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let params = sso_params(port, None);
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        mock_handshake(&mut stream, TOKEN).await;
        serve_sign_on(&mut stream, &recorded_connect("lobby")).await;
        let _ = mock_read_framed_login_packet(&mut stream).await;
        stream.write_all(&[35]).await.unwrap();
    });
    let error = login_lobby(&params).await.expect_err("reply 35 is final");
    server.await.unwrap();
    assert_eq!(
        error.downcast_ref::<LoginReplyState>().map(|s| s.reply),
        Some(35)
    );
}

/// A reconnect the server resumes in place answers reply 15 and the
/// player-positions block; the worker returns it and reads nothing after it.
#[tokio::test]
async fn reply_15_resumes_the_session_in_place() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let block: Vec<u8> = (0..300u16).map(|at| (at % 251) as u8).collect();
    let sent = block.clone();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        mock_handshake(&mut stream, TOKEN).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut stream).await;
        assert_eq!(opcode, crate::proto::login::GAMELOGIN);
        assert_eq!(body[8], 1, "the login says it is a reconnect");
        let mut reply = vec![15];
        reply.extend((sent.len() as u16).to_be_bytes());
        reply.extend(&sent);
        // The first game packet is already on its way.
        reply.push(0xAB);
        stream.write_all(&reply).await.unwrap();
        stream
    });
    let params = LoginParams {
        auth: AuthOptions {
            reconnect: true,
            ..AuthOptions::default()
        },
        ..LoginParams::new("127.0.0.1", port, "test", "")
    };
    let (mut stream, ok) = login_world(&params).await.unwrap();
    let _server_stream = server.await.unwrap();
    assert_eq!(ok.resume.as_deref(), Some(&block[..]));
    assert_eq!(ok.pid, None);
    assert!(ok.server_varcs.is_empty());
    let mut next = [0u8; 1];
    stream.read_exact(&mut next).await.unwrap();
    assert_eq!(next[0], 0xAB, "the login stops at the end of the block");
}

/// The lobby has no world to resume: reply 15 there is an unexpected reply.
#[tokio::test]
async fn a_lobby_has_no_in_place_reconnect() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        mock_handshake(&mut stream, TOKEN).await;
        let _ = mock_read_framed_login_packet(&mut stream).await;
        stream.write_all(&[15, 0, 0]).await.unwrap();
    });
    let error = login_lobby(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .expect_err("reply 15 ends a lobby login");
    server.await.unwrap();
    assert_eq!(
        error.downcast_ref::<LoginReplyState>().map(|s| s.reply),
        Some(15)
    );
}

/// A world that fails a login only after accepting it (at
/// `GAMELOGIN_CONTINUE`) is marked so a lobby login falls back to the lobby by
/// logging out; a refusal of the login itself is not.
#[tokio::test]
async fn a_world_login_that_fails_after_it_was_accepted_is_marked() {
    for (reply, late) in [(vec![5u8], true), (vec![45, 7, 0, 9], true)] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let sent = reply.clone();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            mock_handshake(&mut stream, TOKEN).await;
            let _ = mock_read_framed_login_packet(&mut stream).await;
            stream.write_all(&[2, 0, 1, 1]).await.unwrap();
            let mut cont = [0u8; 1];
            stream.read_exact(&mut cont).await.unwrap();
            stream.write_all(&sent).await.unwrap();
        });
        let error = login_world(&LoginParams::new("127.0.0.1", port, "test", "secret"))
            .await
            .expect_err("the world refuses");
        server.await.unwrap();
        assert_eq!(error.downcast_ref::<LateLoginFailure>().is_some(), late);
        match reply[0] {
            5 => assert_eq!(
                error.downcast_ref::<LoginReplyState>().map(|s| s.reply),
                Some(5)
            ),
            _ => assert_eq!(
                error.downcast_ref::<LoginTransferFailure>(),
                Some(&LoginTransferFailure {
                    reply: 45,
                    disallow_result: 7,
                    disallow_trigger: 9
                })
            ),
        }
    }
    // The first decision refuses the login itself: not late.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        mock_handshake(&mut stream, TOKEN).await;
        let _ = mock_read_framed_login_packet(&mut stream).await;
        stream.write_all(&[5]).await.unwrap();
    });
    let error = login_world(&LoginParams::new("127.0.0.1", port, "test", "secret"))
        .await
        .err()
        .unwrap();
    server.await.unwrap();
    assert!(error.downcast_ref::<LateLoginFailure>().is_none());
}

/// The game login flags a world switch: the byte before the lobby node is 0
/// for the world the lobby advertised and 1 otherwise.
#[test]
fn the_login_flags_a_world_switch() {
    let mut params = LoginParams::new("127.0.0.1", 0, "test", "secret");
    // The layout ends: switch flag, lobby node (2 bytes), 41 checksums.
    let flag_at = |payload: &[u8]| payload[payload.len() - 41 * 4 - 3];
    params.switched_world = false;
    assert_eq!(flag_at(&game_login_payload(&params, 0).unwrap()), 0);
    params.switched_world = true;
    assert_eq!(flag_at(&game_login_payload(&params, 0).unwrap()), 1);
}

fn short_clock(limit_ticks: u64) -> LoginClock {
    let mut clock = LoginClock::new(false);
    clock.limit_ticks = limit_ticks;
    clock
}

/// Reply 1 parks the login for the advertisement countdown: nothing more is
/// read and no time passes until `login_continue`; the login step after it
/// reads nothing, so `loginwait` runs out and the attempt reconnects. A
/// `login_continue` while nothing is parked is ignored.
#[tokio::test]
async fn reply_1_parks_the_login_until_it_is_continued() {
    let (mut endpoint, mut peer) = tokio::io::duplex(16);
    peer.write_all(&[1]).await.unwrap();
    let progress = LoginProgress::new();
    progress.request_resume();
    assert!(!progress.take_resume(), "nothing was parked");
    let waiting = Arc::clone(&progress);
    let decision = tokio::spawn(async move {
        let mut clock = short_clock(4);
        clock.enter_step98();
        clock.limit_ticks = 4;
        let (mut offset, mut last) = (0, None);
        read_login_decision(
            &mut endpoint,
            policy("world", false),
            &mut clock,
            &mut offset,
            &mut last,
            &waiting,
        )
        .await
    });
    let parked = std::time::Instant::now();
    while progress.interim_reply() != Some(1) {
        assert!(parked.elapsed() < Duration::from_secs(5), "never parked");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !decision.is_finished(),
        "a parked login waits for the player"
    );
    progress.request_resume();
    let outcome = decision.await.unwrap().unwrap();
    assert!(
        matches!(outcome, Attempt::RetryAfter { timeout: true, .. }),
        "loginwait expires after the login is continued"
    );
}

/// The device check holds a lobby login: replies 49 and 52 keep it waiting,
/// the page of reply 52 goes to the login screen, requests the screen queues
/// go out on the waiting connection, and the reply that follows decides.
#[tokio::test]
async fn the_device_check_holds_a_lobby_login() {
    let (mut endpoint, mut peer) = tokio::io::duplex(64);
    let progress = LoginProgress::new();
    let waiting = Arc::clone(&progress);
    let decision = tokio::spawn(async move {
        let mut clock = LoginClock::new(false);
        clock.enter_step98();
        let (mut offset, mut last) = (0, None);
        read_login_decision(
            &mut endpoint,
            policy("lobby", true),
            &mut clock,
            &mut offset,
            &mut last,
            &waiting,
        )
        .await
    });
    peer.write_all(&[49]).await.unwrap();
    let held = std::time::Instant::now();
    while progress.interim_reply() != Some(49) {
        assert!(held.elapsed() < Duration::from_secs(5), "not held");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    // "Resend email": the packet the login screen queued reaches the server.
    progress.queue_packet(encode_uid_passport_resend_request());
    let mut request = [0u8; 1];
    peer.read_exact(&mut request).await.unwrap();
    assert_eq!(
        request[0],
        crate::proto::client::UID_PASSPORT_RESEND_REQUEST
    );
    // The validation page, then the device is accepted.
    let page = b"https://example.test/validate";
    let mut reply = vec![52];
    reply.extend(((page.len() + 1) as u16).to_be_bytes());
    reply.extend(page);
    reply.push(0);
    peer.write_all(&reply).await.unwrap();
    let opened = std::time::Instant::now();
    while progress.interim_reply() != Some(52) {
        assert!(opened.elapsed() < Duration::from_secs(5), "page not read");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        !decision.is_finished(),
        "reply 52 keeps a lobby login waiting"
    );
    peer.write_all(&[2]).await.unwrap();
    let outcome = decision.await.unwrap().unwrap();
    assert!(matches!(outcome, Attempt::Done(Decision::Proceed)));
    assert_eq!(progress.take_urls(), ["https://example.test/validate"]);
}

/// The relogin after a logout waits for nothing: the device check ends it
/// (reply 49), and reply 52 opens its page and ends it.
#[tokio::test]
async fn the_device_check_ends_a_relogin() {
    for (reply, page) in [
        (vec![49u8], None),
        (vec![52, 0, 4, b'h', b'i', b'!', 0], Some("hi!")),
    ] {
        let (mut endpoint, mut peer) = tokio::io::duplex(64);
        peer.write_all(&reply).await.unwrap();
        let progress = LoginProgress::new();
        let mut clock = LoginClock::new(false);
        clock.enter_step98();
        let (mut offset, mut last) = (0, None);
        let error = read_login_decision(
            &mut endpoint,
            ReplyPolicy {
                holds: false,
                ..policy("lobby", true)
            },
            &mut clock,
            &mut offset,
            &mut last,
            &progress,
        )
        .await
        .expect_err("the relogin ends");
        assert_eq!(
            error.downcast_ref::<LoginReplyState>().map(|s| s.reply),
            Some(i32::from(reply[0]))
        );
        let urls = progress.take_urls();
        assert_eq!(urls.first().map(String::as_str), page);
    }
}

// -- The blocks the development server's tests decode ----------------------

/// The key the development server's social provider gives network 6, and the
/// base 37 word of its account name "social6".
const DEV_SOCIAL_KEY: i64 = 0x5A00_0000_0000_0006;
const DEV_SOCIAL_NAME: i64 = 49_795_041_332;

fn social_variants_path() -> std::path::PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/social_login_variants.json")
}

/// The login blocks a social sign-on and a world hop write, with server token
/// 7: what the dev server's tests decode and replay over real sockets.
fn social_variants_json() -> String {
    let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
    let signed_on = LoginParams {
        sso: Some(SsoLogin {
            network: 6,
            social_key: Some(DEV_SOCIAL_KEY),
            social_name: DEV_SOCIAL_NAME,
        }),
        ..LoginParams::new("127.0.0.1", 0, "", "")
    };
    let signing_on = LoginParams {
        sso: Some(SsoLogin {
            network: 6,
            social_key: None,
            social_name: 0,
        }),
        ..LoginParams::new("127.0.0.1", 0, "", "")
    };
    let reconnecting = LoginParams {
        auth: AuthOptions {
            reconnect: true,
            ..AuthOptions::default()
        },
        ..signing_on.clone()
    };
    let advertised = LoginParams {
        switched_world: false,
        ..LoginParams::new("127.0.0.1", 0, "Alice", "alice-password")
    };
    let row = |name: &str, kind: &str, bytes: Vec<u8>, extra: serde_json::Value| {
        let mut row = serde_json::json!({"name": name, "kind": kind, "hex": hex(&bytes)});
        row.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        row
    };
    let rows = vec![
        row(
            "game-known-key",
            "game",
            game_login_payload(&signed_on, 7).unwrap(),
            serde_json::json!({"key": DEV_SOCIAL_KEY.to_string(), "nameWord": DEV_SOCIAL_NAME.to_string(), "switchedWorld": true}),
        ),
        row(
            "lobby-known-key",
            "lobby",
            lobby_login_payload(&signed_on, 7).unwrap(),
            serde_json::json!({"key": DEV_SOCIAL_KEY.to_string(), "nameWord": DEV_SOCIAL_NAME.to_string()}),
        ),
        row(
            "game-advertised-world",
            "game",
            game_login_payload(&advertised, 7).unwrap(),
            serde_json::json!({"username": "alice", "switchedWorld": false}),
        ),
        row(
            "lobby-fresh",
            "lobby",
            lobby_login_payload(
                &LoginParams::new("127.0.0.1", 0, "Alice", "alice-password"),
                7,
            )
            .unwrap(),
            serde_json::json!({"username": "alice"}),
        ),
        row(
            "connection-world",
            "connection-world",
            social_connection_payload(ConnectionKind::World, &signing_on, 7).unwrap(),
            serde_json::json!({"network": 6, "reconnect": false}),
        ),
        row(
            "connection-world-reconnect",
            "connection-world",
            social_connection_payload(ConnectionKind::World, &reconnecting, 7).unwrap(),
            serde_json::json!({"network": 6, "reconnect": true}),
        ),
        row(
            "connection-lobby",
            "connection-lobby",
            social_connection_payload(ConnectionKind::Lobby, &signing_on, 7).unwrap(),
            serde_json::json!({"network": 6, "reconnect": false}),
        ),
        row(
            "social-world",
            "social-world",
            social_login_payload(ConnectionKind::World, &signing_on).unwrap(),
            serde_json::json!({"switchedWorld": true}),
        ),
        row(
            "social-lobby",
            "social-lobby",
            social_login_payload(ConnectionKind::Lobby, &signing_on).unwrap(),
            serde_json::json!({}),
        ),
    ];
    let doc = serde_json::json!({
        "about": "Login blocks of a social network sign-on and a world switch, with server token 7 (net::social_connection_payload, social_login_payload, game_login_payload, lobby_login_payload); decoded by server/src/lostcity/network/SocialLogin.test.ts and replayed by WorldSession.integration.test.ts.",
        "regenerate": "cd tools/client910 && cargo test --lib net::tests::session_tests::regenerate_social_login_variants -- --ignored",
        "cases": rows,
    });
    serde_json::to_string_pretty(&doc).unwrap() + "\n"
}

#[test]
fn social_login_variants_fixture_is_current() {
    let fixture = std::fs::read_to_string(social_variants_path()).unwrap();
    assert_eq!(
        fixture,
        social_variants_json(),
        "fixture is stale; regenerate it (see its `regenerate` field)"
    );
}

#[test]
#[ignore = "fixture regeneration"]
fn regenerate_social_login_variants() {
    std::fs::write(social_variants_path(), social_variants_json()).unwrap();
}
