//! The client packets the session sends on its own account, against what the
//! original client wrote for the same inputs (`recorded/network-packets`).
use super::*;

fn unhex(hex: &str) -> Vec<u8> {
    (0..hex.len() / 2)
        .map(|at| u8::from_str_radix(&hex[at * 2..at * 2 + 2], 16).unwrap())
        .collect()
}

/// The recorded lines whose first word is `kind`: the numbers between the
/// kind and the hex bytes, and the bytes.
fn recorded(kind: &str) -> Vec<(Vec<i64>, Vec<u8>)> {
    rs910_core::test_support::frozen::text("network-packets/client-packets.txt")
        .lines()
        .filter_map(|line| {
            let mut words = line.split(' ');
            (words.next() == Some(kind)).then(|| {
                let mut words: Vec<&str> = words.collect();
                let bytes = unhex(words.pop().unwrap());
                let args = words
                    .iter()
                    .map(|word| match *word {
                        "true" => 1,
                        "false" => 0,
                        number => number.parse().unwrap(),
                    })
                    .collect();
                (args, bytes)
            })
        })
        .collect()
}

#[test]
fn verify_id_packet_matches_the_recording() {
    let mut seen = 0;
    for kind in ["verify-id-game", "verify-id-lobby"] {
        for (args, bytes) in recorded(kind) {
            assert_eq!(encode_transmitvar_verifyid(args[0] as i32), bytes);
            seen += 1;
        }
    }
    assert_eq!(seen, 10);
}

#[test]
fn ping_statistics_packet_matches_the_recording() {
    let lines = recorded("ping-statistics");
    assert_eq!(lines.len(), 5);
    for (args, bytes) in lines {
        assert_eq!(
            encode_ping_statistics(args[0] as i32, args[1] as i32, args[2] as i32),
            bytes,
            "{args:?}"
        );
    }
}

#[test]
fn map_build_stuck_packet_matches_the_recording() {
    let lines = recorded("map-build-stuck");
    assert_eq!(lines.len(), 4);
    for (args, bytes) in lines {
        let js5 = Js5Stall {
            connect_state: args[3] as i32,
            error_count: args[4] as i32,
            js5_state: args[5] as i32,
            urgents_full: args[6] != 0,
            prefetches_full: args[7] != 0,
            pending_requests: args[8] as i32,
        };
        assert_eq!(
            encode_map_build_stuck(args[0] as i32, args[1] as i32, args[2] as i32, &js5),
            bytes,
            "{args:?}"
        );
    }
}

/// The login block the original client sent for a launcher's parameters
/// (`recorded/network-packets/login-bodies.txt`): the same body, byte for
/// byte, as the builders write for those parameters. The window, saved
/// options, uid and site settings the original reported are read back out of
/// the recording; everything else is rebuilt.
#[test]
fn login_blocks_match_the_recording() {
    const TOKEN: i64 = 0x0A0B_0C0D_0E0F_1011;
    let lines = rs910_core::test_support::frozen::text("network-packets/login-bodies.txt");
    let mut seen = Vec::new();
    for line in lines.lines() {
        let (name, hex) = line.split_once(' ').unwrap();
        let packet = unhex(hex);
        let (set, kind) = name.split_once('-').unwrap();
        let world = kind == "world";
        // The framing, then the revision words (and the world's reconnect
        // flag), the 52-byte block of the fixed password and the identity.
        let body = &packet[3..];
        let window = if world { 9 + 52 + 6 } else { 8 + 52 + 6 + 2 };
        let at = |from: usize, len: usize| body[from..from + len].to_vec();
        let site_settings = "site=foo";
        let after_uid = window + 6 + 24 + site_settings.len() + 1;
        let prefs_len = usize::from(body[after_uid + if world { 4 } else { 0 }]);
        let prefs_at = after_uid + if world { 5 } else { 1 };
        let launcher = if set == "default" {
            LauncherReport {
                build: -1,
                ..LauncherReport::default()
            }
        } else {
            LauncherReport {
                language: 1,
                game: 0,
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
            }
        };
        // The recorded run's archive checksums: archives 3, 6, 9, ... never
        // loaded; the others a word from their place in the archive list
        // (which counts the loading sprites, archive 32).
        let mut archive_checksums = [0; ARCHIVE_CHECKSUM_WORDS];
        if set == "launcher" {
            for (word, slot) in archive_checksums.iter_mut().enumerate() {
                let place = word + 1 + usize::from(word >= 27);
                if place % 3 != 0 {
                    let spread = if place % 2 == 0 { 0x3000_0000_i32 } else { 0 };
                    *slot = 0x1000_0000 + place as i32 * 0x0101_0101 - spread;
                }
            }
        }
        let params = LoginParams {
            username: "user".into(),
            password: "p\u{e9}ss".into(),
            site_settings: site_settings.into(),
            uid192: body[window + 6..window + 6 + 24].try_into().unwrap(),
            client: ClientReport {
                window_mode: body[window],
                canvas: [
                    u16::from_be_bytes([body[window + 1], body[window + 2]]),
                    u16::from_be_bytes([body[window + 3], body[window + 4]]),
                ],
                anti_aliasing: body[window + 5],
                preferences: at(prefs_at, prefs_len),
            },
            launcher,
            verify_id: if set == "launcher" { 0x0102_0304 } else { 0 },
            archive_checksums,
            switched_world: true,
            ..LoginParams::new("127.0.0.1", 0, "user", "p\u{e9}ss")
        };
        let ours = if world {
            build_game_login_packet(&params, TOKEN).unwrap()
        } else {
            build_lobby_login_packet(&params, TOKEN).unwrap()
        };
        let first = ours.iter().zip(&packet).position(|(a, b)| a != b);
        assert!(
            ours == packet,
            "{name}: first difference at {first:?} (ours {} bytes, original {})",
            ours.len(),
            packet.len()
        );
        seen.push(name.to_owned());
    }
    assert_eq!(
        seen,
        [
            "default-world",
            "default-lobby",
            "launcher-world",
            "launcher-lobby"
        ]
    );
}
