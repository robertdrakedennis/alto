//! Login with the cryptography on. A scripted server holds the TEST-ONLY
//! private key (`fixtures/recorded/login-crypto`), opens what the client
//! sends the way a server does and masks its own frames; the first tests pin
//! the sealing against the packets the original client wrote with the same
//! key.
use super::*;
use crate::login_crypto::LoginSecurity;
use rs910_core::test_support::frozen;
use rs910_protocol::isaac_cipher::IsaacCipher;
use rs910_protocol::rsa_block::test_support::TestKeyPair;
use rs910_protocol::tiny_cipher;
use rs910_protocol::wire_cipher::SessionSeeds;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const TOKEN: i64 = 0x0A0B_0C0D_0E0F_1011;

fn key_pair() -> TestKeyPair {
    TestKeyPair::from_file_text(&frozen::text("login-crypto/TEST-ONLY-rsa-key.json"))
}

fn encrypted(seeds: &[i32]) -> LoginCrypto {
    LoginCrypto::Rsa(Arc::new(LoginSecurity::scripted(
        key_pair().public,
        seeds.iter().copied(),
    )))
}

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
        .collect()
}

fn seeds_of(block: &[u8]) -> SessionSeeds {
    assert_eq!(block[0], 10, "the block's marker");
    std::array::from_fn(|i| i32::from_be_bytes(block[1 + i * 4..5 + i * 4].try_into().unwrap()))
}

/// What a server does with an encrypted login body whose RSA block starts at
/// `rsa_offset`: the plain layout (the block's plaintext in place, the rest
/// decrypted when `tiny_rest`) and the session seeds.
fn open_body(
    key: &TestKeyPair,
    payload: &[u8],
    rsa_offset: usize,
    tiny_rest: bool,
) -> (Vec<u8>, SessionSeeds) {
    let (cipher, rest) = TestKeyPair::split_block(&payload[rsa_offset..]);
    let block = key.decrypt(cipher);
    let seeds = seeds_of(&block);
    let mut rest = rest.to_vec();
    if tiny_rest {
        let end = rest.len();
        tiny_cipher::decrypt_range(&mut rest, 0, end, &seeds);
    }
    let mut plain = payload[..rsa_offset].to_vec();
    plain.extend_from_slice(&block);
    plain.extend_from_slice(&rest);
    (plain, seeds)
}

/// One record of the original client's encrypted packets.
struct Recorded {
    case: String,
    tag: String,
    seeds: SessionSeeds,
    wire: Vec<u8>,
    /// The RSA block's plaintext, where the packet has one.
    block: Option<Vec<u8>>,
}

fn recorded_logins() -> Vec<Recorded> {
    frozen::text("login-crypto/logins.txt")
        .lines()
        .filter(|line| !line.starts_with('#') && !line.starts_with("create-request-"))
        .filter(|line| {
            line.contains(" login ")
                || line.contains("social-connect")
                || line.starts_with("create-connect")
        })
        .map(|line| {
            let mut notes = line.split(" | ");
            let head: Vec<&str> = notes.next().unwrap().split(' ').collect();
            let (case, tag, fields) = if head[0] == "create-connect" {
                ("", head[0], &head[1..])
            } else {
                (head[0], head[1], &head[2..])
            };
            let seeds: Vec<i32> = fields[0]
                .split(',')
                .map(|h| u32::from_str_radix(h, 16).unwrap() as i32)
                .collect();
            let block = notes.find_map(|n| n.strip_prefix("plain ")).map(unhex);
            Recorded {
                case: case.to_owned(),
                tag: tag.to_owned(),
                seeds: seeds.try_into().unwrap(),
                wire: unhex(fields[1]),
                block,
            }
        })
        .collect()
}

/// Sealing the plain layout of each login the original client wrote
/// reproduces its packet byte for byte: where the RSA block starts and ends,
/// the length prefix, the tiny cipher over the rest, and the sign-on's body
/// under the tiny cipher alone.
#[test]
fn sealing_reproduces_the_original_clients_login_packets() {
    let key = key_pair();
    let security = LoginSecurity::scripted(key.public.clone(), []);
    let records = recorded_logins();
    assert_eq!(
        records.len(),
        11,
        "lobby, five world logins, two sign-ons (opening and login), account creation"
    );
    for record in &records {
        let payload = &record.wire[3..];
        let name = format!("{} {}", record.case, record.tag);
        let sealed = match (&record.block, record.tag.as_str()) {
            (Some(block), "login") => {
                let header = if record.case.starts_with("world") {
                    9
                } else {
                    8
                };
                let (plain, seeds) = open_body(&key, payload, header, true);
                assert_eq!(seeds, record.seeds, "{name}");
                let rsa_end = header + block.len();
                security
                    .seal(&plain, Some(header..rsa_end), Some(rsa_end), record.seeds)
                    .unwrap()
            }
            (Some(block), "social-connect") => {
                let header = if record.case.starts_with("world") {
                    9
                } else {
                    8
                };
                let (plain, _) = open_body(&key, payload, header, false);
                security
                    .seal(
                        &plain,
                        Some(header..header + block.len()),
                        None,
                        record.seeds,
                    )
                    .unwrap()
            }
            (Some(block), "create-connect") => {
                let (plain, _) = open_body(&key, payload, 4, true);
                let rsa_end = 4 + block.len();
                security
                    .seal(&plain, Some(4..rsa_end), Some(rsa_end), record.seeds)
                    .unwrap()
            }
            (None, "login") => {
                // A sign-on's login: all of it under the tiny cipher.
                let mut plain = payload.to_vec();
                let end = plain.len();
                tiny_cipher::decrypt_range(&mut plain, 0, end, &record.seeds);
                security.seal(&plain, None, Some(0), record.seeds).unwrap()
            }
            other => panic!("unexpected record {name}: {:?}", other.1),
        };
        assert_eq!(sealed, payload, "{name}");
    }
}

/// The builders put the seeds, the token and the credentials in the RSA
/// block, and the identity and tail of the plain body behind it, for a fresh
/// login, a lobby login and a reconnect (which repeats the previous seeds).
#[test]
fn builders_seal_the_plain_layout() {
    let key = key_pair();
    let plain_params = LoginParams::new("127.0.0.1", 1, "alice", "secret");
    let seed_bytes = |seeds: [i32; 4]| {
        seeds
            .iter()
            .flat_map(|s| s.to_be_bytes())
            .collect::<Vec<u8>>()
    };

    // A fresh world login, then a reconnect: the second block repeats the first's seeds.
    let crypto = encrypted(&[1, -2, 3, 0x7fff_ffff, 5, 6, 7, 8]);
    let fresh = LoginParams {
        crypto: crypto.clone(),
        ..plain_params.clone()
    };
    let sealed = game_login_payload(&fresh, TOKEN).unwrap();
    let (opened, seeds) = open_body(&key, &sealed, 9, true);
    assert_eq!(seeds, [1, -2, 3, 0x7fff_ffff]);
    let mut expected = game_login_payload(&plain_params, TOKEN).unwrap();
    expected[10..26].copy_from_slice(&seed_bytes(seeds));
    assert_eq!(opened, expected);

    let mut reconnecting = LoginParams {
        crypto,
        ..plain_params.clone()
    };
    reconnecting.auth.reconnect = true;
    let sealed = game_login_payload(&reconnecting, TOKEN).unwrap();
    let (opened, seeds) = open_body(&key, &sealed, 9, true);
    assert_eq!(seeds, [5, 6, 7, 8]);
    let mut plain_reconnect = plain_params.clone();
    plain_reconnect.auth.reconnect = true;
    let mut expected = game_login_payload(&plain_reconnect, TOKEN).unwrap();
    expected[10..26].copy_from_slice(&seed_bytes(seeds));
    expected[34..50].copy_from_slice(&seed_bytes([1, -2, 3, 0x7fff_ffff]));
    assert_eq!(opened, expected);

    // The lobby login has the same block behind eight bytes of header.
    let lobby = LoginParams {
        crypto: encrypted(&[9, 10, 11, 12]),
        ..plain_params.clone()
    };
    let sealed = lobby_login_payload(&lobby, TOKEN).unwrap();
    let (opened, seeds) = open_body(&key, &sealed, 8, true);
    assert_eq!(seeds, [9, 10, 11, 12]);
    let mut expected = lobby_login_payload(&plain_params, TOKEN).unwrap();
    expected[9..25].copy_from_slice(&seed_bytes(seeds));
    assert_eq!(opened, expected);
}

/// A login without a usable key fails with the reason instead of sending a
/// block no server can read.
#[tokio::test]
async fn a_missing_key_fails_the_login_with_its_reason() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut init = [0u8; 1];
            let _ = stream.read_exact(&mut init).await;
            let mut reply = vec![0u8];
            reply.extend_from_slice(&TOKEN.to_be_bytes());
            let _ = stream.write_all(&reply).await;
        }
    });
    let params = LoginParams {
        crypto: LoginCrypto::Unavailable("no login RSA key: test".into()),
        ..LoginParams::new("127.0.0.1", port, "alice", "secret")
    };
    let error = login_world(&params).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("no login RSA key"),
        "{error:#}"
    );
}

fn masked(frames: &[(u8, Vec<u8>)], seeds: SessionSeeds) -> (Vec<u8>, Vec<u8>) {
    let mut generator = IsaacCipher::inbound_from_seeds(seeds);
    let (mut wire, mut plain) = (Vec::new(), Vec::new());
    for (opcode, payload) in frames {
        let prefix = match crate::proto::server::size(*opcode).unwrap() {
            -1 => vec![payload.len() as u8],
            -2 => (payload.len() as u16).to_be_bytes().to_vec(),
            _ => vec![],
        };
        // From 128 up an opcode takes two bytes: 128 plus its high part, then its low part.
        let head: Vec<u8> = if *opcode < 128 {
            vec![*opcode]
        } else {
            vec![128, *opcode]
        };
        plain.extend(&head);
        plain.extend(&prefix);
        plain.extend(payload);
        wire.extend(head.iter().map(|b| b.wrapping_add(generator.next_byte())));
        wire.extend(&prefix);
        wire.extend(payload);
    }
    (wire, plain)
}

/// Reply 2 and the var blocks (one final, empty block); the continuation
/// follows the client's continue byte.
fn world_success_reply() -> Vec<u8> {
    vec![2, 0, 1, 1]
}

/// A world login against a server holding the private key: the block opens to
/// the plain body, the continuation works, and from then on the server's
/// masked frames arrive plain while the client's frames leave masked, in
/// sequence with a frame written from another thread's handle.
#[tokio::test]
async fn world_login_round_trip_masks_both_directions() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seeds = [101, -102, 103, 104];
    let params = LoginParams {
        crypto: encrypted(&seeds),
        ..LoginParams::new("127.0.0.1", port, "test", "secret")
    };
    let mut expected_plain = game_login_payload(
        &LoginParams::new("127.0.0.1", port, "test", "secret"),
        0x1112_1314_1516_1718,
    )
    .unwrap();
    expected_plain[10..26].copy_from_slice(
        &seeds
            .iter()
            .flat_map(|s| s.to_be_bytes())
            .collect::<Vec<u8>>(),
    );
    let var_opcode = (0u8..128)
        .find(|op| crate::proto::server::size(*op) == Some(-1))
        .unwrap();
    let frames = vec![
        (crate::proto::server::NO_TIMEOUT, vec![]),
        (crate::proto::server::SERVER_TICK_END, vec![]),
        (var_opcode, vec![1, 2, 3]),
    ];
    let (wire_frames, plain_frames) = masked(&frames, seeds);
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        mock_handshake(&mut s, 0x1112_1314_1516_1718).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut s).await;
        assert_eq!(opcode, crate::proto::login::GAMELOGIN);
        let (plain, got_seeds) = open_body(&key_pair(), &body, 9, true);
        assert_eq!(got_seeds, seeds);
        assert_eq!(plain, expected_plain);
        s.write_all(&world_success_reply()).await.unwrap();
        let mut cont = [0u8; 1];
        s.read_exact(&mut cont).await.unwrap();
        assert_eq!(
            cont[0],
            crate::proto::login::GAMELOGIN_CONTINUE,
            "sent plain"
        );
        // reply 2, then the continuation block (auth flag 0, as in the plain mocks).
        let mut block = vec![0u8, 2, 0, 0, 0, 1, 0];
        block.extend_from_slice(&7u16.to_be_bytes());
        block.extend_from_slice(&[1, 0, 0, 0, 1, 0]);
        block.extend_from_slice(&[0; 6]);
        let mut reply = vec![2, block.len() as u8];
        reply.extend_from_slice(&block);
        reply.extend_from_slice(&wire_frames);
        s.write_all(&reply).await.unwrap();
        // The client's masked frames.
        let mut wire = [0u8; 7];
        s.read_exact(&mut wire).await.unwrap();
        let mut generator = IsaacCipher::from_seeds(seeds);
        let opcodes = [wire[0], wire[1], wire[2]].map(|b| b.wrapping_sub(generator.next_byte()));
        assert_eq!(opcodes, [103, 103, 79]);
        assert_eq!(&wire[3..], &[0, 0, 0, 5], "payload bytes stay as they are");
    });
    let (mut stream, ok) = login_world(&params).await.unwrap();
    assert_eq!(ok.pid, Some(7));
    let mut got = vec![0u8; plain_frames.len()];
    stream.read_exact(&mut got).await.unwrap();
    assert_eq!(got, plain_frames, "the server's masked frames read plain");
    stream
        .write_all(&crate::net::encode_no_timeout())
        .await
        .unwrap();
    stream
        .write_all(&crate::net::encode_no_timeout())
        .await
        .unwrap();
    stream
        .write_all(&crate::net::encode_map_build_complete(5))
        .await
        .unwrap();
    server.await.unwrap();
}

/// The profile reply of the plain lobby mock with the authenticator key
/// flag set: the four key bytes that follow are masked values of the
/// server's generator.
fn lobby_profile(auth_key: Option<[u8; 4]>) -> Vec<u8> {
    let mut profile = match auth_key {
        Some(key) => {
            let mut v = vec![1];
            v.extend(key);
            v
        }
        None => vec![0],
    };
    profile.extend_from_slice(&[0, 5, 1, 0, 0, 1, 0, 1, 0]);
    profile.extend_from_slice(&60_000_i64.to_be_bytes());
    profile.extend_from_slice(&[0, 0, 0, 0, 0, 3]);
    profile.extend_from_slice(&5678_i32.to_be_bytes());
    profile.extend_from_slice(&1234_i32.to_be_bytes());
    profile.extend_from_slice(&17_u16.to_be_bytes());
    profile.extend_from_slice(&3_u16.to_be_bytes());
    profile.extend_from_slice(&42_u16.to_be_bytes());
    profile.extend_from_slice(&0_i32.to_be_bytes());
    profile.push(1);
    profile.extend_from_slice(&23_u16.to_be_bytes());
    profile.extend_from_slice(&29_u16.to_be_bytes());
    profile.push(1);
    profile.extend_from_slice(&[0, b't', b'e', b's', b't', 0]);
    profile.push(4);
    profile.extend_from_slice(&12345_i32.to_be_bytes());
    profile.extend_from_slice(&1_u16.to_be_bytes());
    profile.extend_from_slice(&[0, b'l', b'o', b'b', b'b', b'y', 0]);
    profile.extend_from_slice(&43594_u16.to_be_bytes());
    profile.extend_from_slice(&43595_u16.to_be_bytes());
    profile
}

/// The generator values the masked parts of a lobby login's replies consume
/// in order: a device-check page address (reply 52, one value per byte), the
/// authenticator key (flag 1, four values) and then the opcodes of the game
/// frames. A client that skipped any of them would read the frames out of
/// step.
#[tokio::test]
async fn lobby_login_keeps_the_generators_in_step_through_its_masked_replies() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seeds = [7, 8, 9, 10];
    let params = LoginParams {
        crypto: encrypted(&seeds),
        ..LoginParams::new("127.0.0.1", port, "test", "secret")
    };
    let progress = params.progress.clone();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        mock_handshake(&mut s, TOKEN).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut s).await;
        assert_eq!(opcode, crate::proto::login::LOBBYLOGIN);
        let (_, got) = open_body(&key_pair(), &body, 8, true);
        assert_eq!(got, seeds);
        let mut generator = IsaacCipher::inbound_from_seeds(seeds);
        // Reply 52: a masked page address, then the login goes on.
        let page = b"http://example.test/device\0";
        let mut reply = vec![52];
        reply.extend_from_slice(&(page.len() as u16).to_be_bytes());
        reply.extend(page.iter().map(|b| b.wrapping_add(generator.next_byte())));
        // Reply 2 with a profile that carries an authenticator key (four masked values).
        let key = [0xde, 0xad, 0xbe, 0xef];
        let masked_key = key.map(|b: u8| b.wrapping_add(generator.next_byte()));
        let profile = lobby_profile(Some(masked_key));
        reply.extend_from_slice(&[2, profile.len() as u8]);
        reply.extend_from_slice(&profile);
        // Then a masked frame.
        reply.push(crate::proto::server::NO_TIMEOUT.wrapping_add(generator.next_byte()));
        s.write_all(&reply).await.unwrap();
        s.flush().await.unwrap();
        // keep the connection open until the client has read
        let mut sink = [0u8; 1];
        let _ = s.read(&mut sink).await;
    });
    let (mut stream, ok) = login_lobby(&params).await.unwrap();
    assert_eq!(ok.lobby.player_name, "test");
    assert_eq!(progress.take_urls(), ["http://example.test/device"]);
    let mut frame = [0u8; 1];
    stream.read_exact(&mut frame).await.unwrap();
    assert_eq!(frame[0], crate::proto::server::NO_TIMEOUT);
    drop(stream);
    server.await.unwrap();
}

/// A social sign-on with encryption: the opening block's seeds key the
/// server's tiny-encrypted replies (page, then the account) and the sign-on's
/// login body, and the same seeds mask the game stream afterwards.
#[tokio::test]
async fn sign_on_replies_and_body_use_the_openings_seeds() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let seeds = [31, 32, 33, 34];
    let params = LoginParams {
        sso: Some(SsoLogin {
            network: 6,
            social_key: None,
            social_name: 0,
        }),
        crypto: encrypted(&seeds),
        ..LoginParams::new("127.0.0.1", port, "", "")
    };
    let expected_body = social_login_payload(ConnectionKind::World, &params).unwrap();
    let progress = params.progress.clone();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        mock_handshake(&mut s, TOKEN).await;
        let (opcode, body) = mock_read_framed_login_packet(&mut s).await;
        assert_eq!(opcode, crate::proto::login::INIT_SOCIAL_NETWORK_CONNECTION);
        let (_, got) = open_body(&key_pair(), &body, 9, false);
        assert_eq!(got, seeds);
        // The page block (a version byte, the address, a NUL), padded to whole cipher blocks.
        let mut page = vec![0u8; 24];
        page[1..1 + 20].copy_from_slice(b"http://sso.test/page");
        tiny_cipher::encrypt_range(&mut page, 0, 24, &seeds);
        let mut account = [
            0x40u8, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x68, 0x69, 0x6a, 0x6b, 0x6c, 0x6d,
            0x6e, 0x6f,
        ];
        tiny_cipher::encrypt_range(&mut account, 0, 16, &seeds);
        s.write_all(&[0, 24]).await.unwrap();
        s.write_all(&page).await.unwrap();
        s.write_all(&[1]).await.unwrap();
        s.write_all(&account).await.unwrap();
        let (opcode, mut body) = mock_read_framed_login_packet(&mut s).await;
        assert_eq!(opcode, crate::proto::login::SOCIAL_NETWORK_LOGIN);
        let end = body.len();
        tiny_cipher::decrypt_range(&mut body, 0, end, &seeds);
        assert_eq!(body, expected_body);
        // Reply 2, the var blocks, continue, and the continuation block.
        s.write_all(&world_success_reply()).await.unwrap();
        let mut cont = [0u8; 1];
        s.read_exact(&mut cont).await.unwrap();
        let mut block = vec![0u8, 2, 0, 0, 0, 1, 0];
        block.extend_from_slice(&5u16.to_be_bytes());
        block.extend_from_slice(&[1, 0, 0, 0, 1, 0]);
        block.extend_from_slice(&[0; 6]);
        let mut reply = vec![2, block.len() as u8];
        reply.extend_from_slice(&block);
        let mut generator = IsaacCipher::inbound_from_seeds(seeds);
        reply.push(crate::proto::server::NO_TIMEOUT.wrapping_add(generator.next_byte()));
        s.write_all(&reply).await.unwrap();
    });
    let (mut stream, ok) = login_world(&params).await.unwrap();
    assert_eq!(ok.pid, Some(5));
    assert_eq!(
        progress.social(),
        Some((0x4041_4243_4445_4647, 0x6869_6a6b_6c6d_6e6f))
    );
    assert_eq!(progress.take_urls(), ["http://sso.test/page"]);
    let mut frame = [0u8; 1];
    stream.read_exact(&mut frame).await.unwrap();
    assert_eq!(frame[0], crate::proto::server::NO_TIMEOUT);
    server.await.unwrap();
}

/// Account creation with encryption: the connect block opens with the
/// seeds in range, the reply installs the cipher, and the first request
/// leaves masked with its body under the tiny cipher.
#[tokio::test]
async fn account_creation_connect_installs_the_cipher_for_its_requests() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let info = CreateConnectInfo {
        crypto: encrypted(&[
            0x4000_0000,
            0x8000_0000_u32 as i32,
            1,
            2,
            3,
            4,
            5,
            6,
            7,
            8,
            9,
            10,
            11,
            12,
            13,
            14,
        ]),
        ..CreateConnectInfo::default()
    };
    let plain_info = CreateConnectInfo::default();
    let server = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let (opcode, body) = mock_read_framed_login_packet(&mut s).await;
        assert_eq!(opcode, crate::proto::login::CREATE_ACCOUNT_CONNECT);
        let (plain, seeds) = open_body(&key_pair(), &body, 4, true);
        assert!(
            seeds.iter().all(|s| (0..99_999_999).contains(s)),
            "{seeds:?}"
        );
        // Behind the block (marker, seeds, ten words, a short) the body is the plain one.
        let plain_packet = create_account_connect_packet(&plain_info).unwrap();
        assert_eq!(&plain[4 + 59..], &plain_packet[3 + 4 + 59..]);
        s.write_all(&[2]).await.unwrap();
        // The first request: opcode masked with the generator, the body tiny-encrypted.
        let mut head = [0u8; 3];
        s.read_exact(&mut head).await.unwrap();
        let mut generator = IsaacCipher::from_seeds(seeds);
        assert_eq!(
            head[0].wrapping_sub(generator.next_byte()),
            crate::proto::client::CREATE_CHECK_EMAIL
        );
        let mut request = vec![0u8; usize::from(u16::from_be_bytes([head[1], head[2]]))];
        s.read_exact(&mut request).await.unwrap();
        let end = request.len();
        tiny_cipher::decrypt_range(&mut request, 0, end, &seeds);
        assert_eq!(request, b"a@b.c\0\0\0\0\0\0\0\0");
    });
    let (mut stream, reply) = create_account_connect_at("127.0.0.1", port, &info)
        .await
        .unwrap();
    assert_eq!(reply, 2);
    assert!(stream.is_masked());
    let mut frame = vec![crate::proto::client::CREATE_CHECK_EMAIL, 0, 13];
    frame.extend_from_slice(b"a@b.c\0\0\0\0\0\0\0\0");
    stream.write_all(&frame).await.unwrap();
    server.await.unwrap();
}
