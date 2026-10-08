use super::*;
use rs910_symbols::{component, interface};

/// The lobby's player-info slot and the pane opened into it.
const PLAYER_INFO_SLOT: i32 = component::lobby_window::PLAYER_INFO_SLOT.packed();
const PLAYER_INFO: u16 = interface::LOBBY_PLAYER_INFO.id() as u16;
/// The game window's world-view slot and the world view.
const WORLD_VIEW_SLOT: i32 = component::game_window::WORLD_VIEW_SLOT.packed();
const WORLD_VIEW: u16 = interface::WORLD_VIEW.id() as u16;
const LOBBY_FRIENDS_CHAT: u16 = interface::LOBBY_FRIENDS_CHAT.id() as u16;
const LOBBY_TOP: u16 = interface::LOBBY_WINDOW.id() as u16;
const GAME_TOP: u16 = interface::GAME_WINDOW.id() as u16;

/// Mirror the server's encode order exactly (verified against
/// `Packet.ts`, NOT plain big-endian).
fn server_rebuild_tail(
    zone_x: u16,
    zone_z: u16,
    npc_bits: u8,
    count: u8,
    build_area_id: u8,
    force: bool,
) -> [u8; 8] {
    let p2_alt2 = |value: u16| [(value >> 8) as u8, value.wrapping_add(128) as u8];
    let zx = p2_alt2(zone_x);
    let zz = p2_alt2(zone_z);
    [
        zx[0],
        zx[1],
        npc_bits,
        128u8.wrapping_sub(count),
        build_area_id,
        zz[0],
        zz[1],
        128u8.wrapping_sub(u8::from(force)),
    ]
}

#[test]
fn rebuild_parse_skips_login_time_high_res_prefix() {
    // Player.ts login() sends with nearbyPlayers=true: 30 bits + 2047 x
    // 18 bits = 36876 bits -> ceil/8 = 4610 prefix bytes, then the tail.
    let tail = server_rebuild_tail(402, 402, 5, 9, 0, true);
    let mut payload = vec![0xABu8; 4610];
    payload.extend_from_slice(&tail);
    let rebuild = parse_rebuild_normal(&payload).unwrap();
    assert_eq!(rebuild.zone_x, 402);
    assert_eq!(rebuild.zone_z, 402);
    assert_eq!(rebuild.build_area_id, 0);
    assert!(rebuild.force);
    assert!(rebuild.has_high_res_block);
    assert_eq!(rebuild.high_res_bytes, 4610);
}

/// Bare 8-byte tails in the server's packet order: the login spawn
/// (Lumbridge 3222,3222 -> zone 402,402) and the World.ts tele cheat
/// resend without nearbyPlayers (asymmetric zone, so an x/z swap shows).
#[test]
fn rebuild_parse_accepts_bare_tails() {
    for (zone_x, zone_z, npc_bits, count, build_area_id, force) in
        [(402, 402, 5, 9, 0, true), (300, 310, 5, 9, 1, false)]
    {
        let tail = server_rebuild_tail(zone_x, zone_z, npc_bits, count, build_area_id, force);
        assert_eq!(
            parse_rebuild_normal(&tail).unwrap(),
            Rebuild {
                zone_x,
                zone_z,
                npc_bits,
                map_count: count,
                build_area_id,
                force,
                has_high_res_block: false,
                high_res_bytes: 0,
            }
        );
    }
    assert!(parse_rebuild_normal(&[]).is_err());
    assert!(parse_rebuild_normal(&[0u8; 7]).is_err());
}

#[test]
fn spawn_zone_maps_to_cached_lumbridge_block() {
    // Spawn 3222 >> 3 = 402 (zone), 402 >> 3 = 50 (mapsquare).
    assert_eq!(3222 >> 3, 402);
    assert_eq!(402 >> 3, 50);
    let rebuild = parse_rebuild_normal(&server_rebuild_tail(402, 402, 5, 9, 0, true)).unwrap();
    let groups = rebuild_to_groups(&rebuild);
    // 3x3 around mapsquare (50, 50): group = mx | mz << 7.
    assert!(groups.contains(&6450));
    let as_u32: Vec<u32> = groups.iter().map(|group| u32::from(*group)).collect();
    assert_eq!(as_u32, crate::map::LUMBRIDGE_GROUPS.to_vec());
    assert!(is_cached_lumbridge_block(&groups));
    assert!(!is_cached_lumbridge_block(&groups[..8]));
}

#[test]
fn groups_clamp_at_map_edge() {
    let rebuild = Rebuild {
        zone_x: 0,
        zone_z: 0,
        npc_bits: 5,
        map_count: 9,
        build_area_id: 0,
        force: true,
        has_high_res_block: false,
        high_res_bytes: 0,
    };
    let groups = rebuild_to_groups(&rebuild);
    // Corner clamps to mapsquares 0..=1, so only 4 distinct groups.
    assert_eq!(groups, vec![0, 1, 128, 129]);
}

/// Drain-loop over a `tokio::io::duplex` mock server feeding framed
/// `NO_TIMEOUT` + `REBUILD_NORMAL` + `PLAYER_INFO` + `SERVER_TICK_END`
/// (framing per `net.rs`: bare opcode for size-0, `u8 opcode + u16 BE
/// len + payload` for `-2`), then assert the replies on the server end.
#[tokio::test]
async fn drain_loop_validates_live_rebuild_then_player_info() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let tail = server_rebuild_tail(402, 402, 5, 9, 0, true);
    let player_payload = vec![0xAAu8; 64];
    let mut frames = Vec::new();
    frames.push(crate::proto::server::NO_TIMEOUT);
    frames.push(crate::proto::server::REBUILD_NORMAL);
    frames.extend_from_slice(&(tail.len() as u16).to_be_bytes());
    frames.extend_from_slice(&tail);
    frames.push(crate::proto::server::PLAYER_INFO);
    frames.extend_from_slice(&(player_payload.len() as u16).to_be_bytes());
    frames.extend_from_slice(&player_payload);
    frames.extend([128, crate::proto::server::SERVER_TICK_END]);

    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    server.write_all(&frames).await.unwrap();

    let outcome = drain_frames(&mut client, Duration::from_secs(5))
        .await
        .unwrap();
    let rebuild = outcome.rebuild.expect("drain must capture REBUILD_NORMAL");
    assert_eq!((rebuild.zone_x, rebuild.zone_z), (402, 402));
    assert!(outcome.player_seen);
    assert_eq!(outcome.player_info_bytes, 64);

    // This headless decoder cannot advertise a window. The server's keepalive
    // is not answered: the only reply is the legacy preview map response.
    let mut replies = vec![0u8; 5];
    tokio::time::timeout(Duration::from_secs(1), server.read_exact(&mut replies))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replies[0], crate::proto::client::MAP_BUILD_COMPLETE);
}

#[tokio::test]
async fn drain_loop_times_out_with_partial_state() {
    let (mut client, _server) = tokio::io::duplex(64 * 1024);
    let outcome = drain_frames(&mut client, Duration::from_millis(50))
        .await
        .unwrap();
    assert!(outcome.rebuild.is_none());
    assert!(!outcome.player_seen);
}

#[test]
fn should_reload_dedups_same_groups() {
    let spawn = parse_rebuild_normal(&server_rebuild_tail(402, 402, 5, 9, 0, true)).unwrap();
    let spawn_groups = rebuild_to_groups(&spawn);
    // Identical (even reordered) repeats are no-ops.
    let mut reordered = spawn_groups.clone();
    reordered.reverse();
    assert!(!should_reload(&spawn_groups, &reordered));
    assert!(!should_reload(&spawn_groups, &spawn_groups));
    // A teleport to a different 3x3 block must reload.
    let away = parse_rebuild_normal(&server_rebuild_tail(300, 310, 5, 9, 0, false)).unwrap();
    let away_groups = rebuild_to_groups(&away);
    assert_ne!(spawn_groups, away_groups);
    assert!(should_reload(&spawn_groups, &away_groups));
    assert!(should_reload(&[], &away_groups));
    assert!(!should_reload(&[], &[]));
}

// --- Live interface / CS2 byte vectors (P0) ---
//
// Encoders mirror the server's packet table and `Packet.ts`
// (`_alt*` shuffles); parsers must invert them exactly.

fn enc_p1_alt1(value: u8) -> u8 {
    value.wrapping_add(128)
}

fn enc_p1_alt2(value: u8) -> u8 {
    (0u8).wrapping_sub(value)
}

fn enc_p1_alt3(value: u8) -> u8 {
    128u8.wrapping_sub(value)
}

fn enc_p2_alt2(value: u16) -> [u8; 2] {
    [(value >> 8) as u8, value.wrapping_add(128) as u8]
}

fn enc_p2_alt1(value: u16) -> [u8; 2] {
    value.to_le_bytes()
}

fn enc_p2_alt3(value: u16) -> [u8; 2] {
    [value.wrapping_add(128) as u8, (value >> 8) as u8]
}

fn enc_p4_alt1(value: i32) -> [u8; 4] {
    value.to_le_bytes()
}

fn enc_p4_alt2(value: i32) -> [u8; 4] {
    let bytes = value.to_be_bytes();
    [bytes[2], bytes[3], bytes[0], bytes[1]]
}

fn enc_p4_alt3(value: i32) -> [u8; 4] {
    let bytes = value.to_be_bytes();
    [bytes[1], bytes[0], bytes[3], bytes[2]]
}

fn enc_p2(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

fn enc_p4(value: i32) -> [u8; 4] {
    value.to_be_bytes()
}

fn enc_str(text: &str) -> Vec<u8> {
    let mut out = text.as_bytes().to_vec();
    out.push(0);
    out
}

#[test]
fn if_opensub_vector_round_trips() {
    // The lobby opens its player-info pane into the player-info slot, type 1
    // (`LoginLayout.ts`).
    let parent = PLAYER_INFO_SLOT;
    let mut payload = Vec::new();
    payload.extend_from_slice(&enc_p4_alt2(2));
    payload.extend_from_slice(&enc_p4_alt1(parent));
    payload.push(enc_p1_alt2(1));
    payload.extend_from_slice(&enc_p4(3));
    payload.extend_from_slice(&enc_p2(PLAYER_INFO));
    payload.extend_from_slice(&enc_p4_alt2(1));
    payload.extend_from_slice(&enc_p4_alt2(0));
    assert_eq!(payload.len(), 23);
    let (got_parent, got_sub, got_kind) = parse_if_opensub(&payload).unwrap();
    assert_eq!(got_parent, parent as u32);
    assert_eq!(got_sub, u32::from(PLAYER_INFO));
    assert_eq!(got_kind, 1);
    assert!(parse_if_opensub(&payload[..22]).is_err());
}

#[test]
fn if_active_variants_round_trip_parent_sub_kind() {
    // ACTIVE_LOC 26/32: parent, coord, sub, type.
    let parent = PLAYER_INFO_SLOT;
    let mut loc = Vec::new();
    loc.extend_from_slice(&enc_p4_alt1(parent));
    loc.extend_from_slice(&enc_p4_alt3(0x01020304));
    loc.extend_from_slice(&enc_p2(PLAYER_INFO));
    loc.push(enc_p1_alt3(1));
    loc.extend_from_slice(&enc_p4_alt3(11));
    loc.extend_from_slice(&enc_p4(12));
    loc.extend_from_slice(&enc_p4_alt2(13));
    loc.extend_from_slice(&enc_p4_alt3(14));
    loc.push(0);
    loc.extend_from_slice(&enc_p4(0));
    assert_eq!(loc.len(), 32);
    let (got_parent, got_sub, got_kind) = parse_if_opensub_active_loc(&loc).unwrap();
    assert_eq!(
        (got_parent, got_sub, got_kind),
        (parent as u32, u32::from(PLAYER_INFO), 1)
    );

    // ACTIVE_PLAYER 61/25: parent is the 3rd field,
    // sub is the trailing `g2_alt3`.
    let p_parent = 0x0A0B0C0Di32;
    let mut player = Vec::new();
    player.extend_from_slice(&enc_p4_alt2(1));
    player.extend_from_slice(&enc_p4_alt3(2));
    player.extend_from_slice(&enc_p4_alt2(p_parent));
    player.extend_from_slice(&enc_p4(3));
    player.push(2);
    player.extend_from_slice(&enc_p2(100));
    player.extend_from_slice(&enc_p4_alt3(4));
    player.extend_from_slice(&enc_p2_alt3(WORLD_VIEW));
    assert_eq!(player.len(), 25);
    let (got_parent, got_sub, got_kind) = parse_if_opensub_active_player(&player).unwrap();
    assert_eq!(
        (got_parent, got_sub, got_kind),
        (p_parent as u32, u32::from(WORLD_VIEW), 2)
    );

    // ACTIVE_NPC 102/25: sub first, parent last.
    let n_parent = 0x11223344i32;
    let mut npc = Vec::new();
    npc.extend_from_slice(&enc_p2_alt3(LOBBY_FRIENDS_CHAT));
    npc.extend_from_slice(&enc_p4_alt1(5));
    npc.extend_from_slice(&enc_p4_alt3(6));
    npc.extend_from_slice(&enc_p4_alt3(7));
    npc.extend_from_slice(&enc_p2(8));
    npc.push(1);
    npc.extend_from_slice(&enc_p4(9));
    npc.extend_from_slice(&enc_p4_alt1(n_parent));
    assert_eq!(npc.len(), 25);
    let (got_parent, got_sub, got_kind) = parse_if_opensub_active_npc(&npc).unwrap();
    assert_eq!(
        (got_parent, got_sub, got_kind),
        (n_parent as u32, u32::from(LOBBY_FRIENDS_CHAT), 1)
    );

    // ACTIVE_OBJ 121/29: sub `g2`, parent `g4_alt1`,
    // type `g1_alt1`.
    let o_parent = WORLD_VIEW_SLOT;
    let mut obj = Vec::new();
    obj.extend_from_slice(&enc_p2(WORLD_VIEW));
    obj.extend_from_slice(&enc_p4_alt3(21));
    obj.extend_from_slice(&enc_p4_alt2(0x02030405));
    obj.extend_from_slice(&enc_p4_alt1(o_parent));
    obj.extend_from_slice(&enc_p4(22));
    obj.extend_from_slice(&enc_p2(23));
    obj.extend_from_slice(&enc_p4_alt3(24));
    obj.push(enc_p1_alt1(1));
    obj.extend_from_slice(&enc_p4_alt2(25));
    assert_eq!(obj.len(), 29);
    let (got_parent, got_sub, got_kind) = parse_if_opensub_active_obj(&obj).unwrap();
    assert_eq!(
        (got_parent, got_sub, got_kind),
        (o_parent as u32, u32::from(WORLD_VIEW), 1)
    );
}

#[test]
fn if_settext_sethide_setposition_setscrollpos_vectors() {
    // IF_SETTEXT 181/-2: `g4_alt1` + `gjstr`.
    let packed = component::lobby_window::TIMER_BACKDROP.packed();
    let mut text_payload = Vec::new();
    text_payload.extend_from_slice(&enc_p4_alt1(packed));
    text_payload.extend_from_slice(&enc_str("Hello"));
    let (got_packed, got_text) = parse_if_settext(&text_payload).unwrap();
    assert_eq!(got_packed, packed as u32);
    assert_eq!(got_text, "Hello");
    assert!(parse_if_settext(&text_payload[..4]).is_err());

    // IF_SETHIDE 109/5: `g4s` + `g1_alt2`.
    let hide_packed = PLAYER_INFO_SLOT;
    let mut hide = Vec::new();
    hide.extend_from_slice(&enc_p4(hide_packed));
    hide.push(enc_p1_alt2(1));
    assert_eq!(hide.len(), 5);
    assert_eq!(parse_if_sethide(&hide).unwrap(), (hide_packed as u32, true));
    let mut show = Vec::new();
    show.extend_from_slice(&enc_p4(hide_packed));
    show.push(enc_p1_alt2(0));
    assert_eq!(
        parse_if_sethide(&show).unwrap(),
        (hide_packed as u32, false)
    );

    // IF_SETPOSITION 72/8: `g2s_alt1` x,
    // `g2s_alt2` y, `g4_alt1` packed.
    let pos_packed = WORLD_VIEW_SLOT;
    let mut pos = Vec::new();
    pos.extend_from_slice(&(-5i16).to_le_bytes());
    pos.extend_from_slice(&enc_p2_alt2(10));
    pos.extend_from_slice(&enc_p4_alt1(pos_packed));
    assert_eq!(pos.len(), 8);
    let (got_packed, got_x, got_y) = parse_if_setposition(&pos).unwrap();
    assert_eq!(got_packed, pos_packed as u32);
    assert_eq!((got_x, got_y), (-5, 10));

    // IF_SETSCROLLPOS 79/6: `g4s` + `g2_alt2`.
    let scroll_packed = WORLD_VIEW_SLOT;
    let mut scroll = Vec::new();
    scroll.extend_from_slice(&enc_p4(scroll_packed));
    scroll.extend_from_slice(&enc_p2_alt2(120));
    assert_eq!(scroll.len(), 6);
    assert_eq!(
        parse_if_setscrollpos(&scroll).unwrap(),
        (scroll_packed as u32, 120)
    );
}

#[test]
fn runclientscript_variable_length_descriptor_round_trips() {
    // Server encode: descriptor built reverse over args,
    // then args forward, then `p4` script id. The decoder reads
    // the descriptor, then reverse-index reads, then `g4s` script id; the
    // returned args are in stored (reverse-of-wire) order.
    // Wire args [int 1, int 2, str "a"] -> descriptor "sii", wire
    // [1,2,"a"], stored ["a",2,1].
    let mut payload = Vec::new();
    payload.extend_from_slice(&enc_str("sii"));
    payload.extend_from_slice(&enc_p4(1));
    payload.extend_from_slice(&enc_p4(2));
    payload.extend_from_slice(&enc_str("a"));
    payload.extend_from_slice(&enc_p4(999));
    let script = parse_runclientscript(&payload).unwrap();
    assert_eq!(script.script_id, 999);
    assert_eq!(
        script.args,
        vec![
            ScriptArg::Str("a".to_string()),
            ScriptArg::Int(2),
            ScriptArg::Int(1),
        ]
    );
    // Two-arg mixed [int 7, str "hello"] -> descriptor "si", stored
    // ["hello",7].
    let mut mixed = Vec::new();
    mixed.extend_from_slice(&enc_str("si"));
    mixed.extend_from_slice(&enc_p4(7));
    mixed.extend_from_slice(&enc_str("hello"));
    mixed.extend_from_slice(&enc_p4(1234));
    let script = parse_runclientscript(&mixed).unwrap();
    assert_eq!(script.script_id, 1234);
    assert_eq!(
        script.args,
        vec![ScriptArg::Str("hello".to_string()), ScriptArg::Int(7),]
    );
    // Empty descriptor: no args, just the script id.
    let mut empty = Vec::new();
    empty.extend_from_slice(&enc_str(""));
    empty.extend_from_slice(&enc_p4(55));
    let script = parse_runclientscript(&empty).unwrap();
    assert_eq!(script.script_id, 55);
    assert!(script.args.is_empty());
    // Truncated args error (descriptor claims an int, no bytes follow).
    let mut short = Vec::new();
    short.extend_from_slice(&enc_str("i"));
    short.extend_from_slice(&enc_p4(1));
    short.truncate(short.len() - 1);
    assert!(parse_runclientscript(&short).is_err());
}

#[test]
fn parse_ui_event_dispatches_all_live_ops() {
    let mut song = Vec::new();
    song.extend_from_slice(&enc_p2_alt3(321));
    song.push(enc_p1_alt2(77));
    assert_eq!(
        parse_ui_event(crate::proto::server::MIDI_SONG, &song).unwrap(),
        Some(UiEvent::Audio {
            command: "midi_song".into(),
            args: vec![321, 77]
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::MIDI_SONG_STOP, &[]).unwrap(),
        Some(UiEvent::Audio {
            command: "midi_song_stop".into(),
            args: vec![]
        })
    );
    assert!(parse_ui_event(crate::proto::server::MIDI_SONG, &song[..2]).is_err());
    let snapshot = [7, 1, 0xaa, 0xbb];
    assert_eq!(
        parse_ui_event(crate::proto::server::PLAYER_SNAPSHOT, &snapshot).unwrap(),
        Some(UiEvent::PlayerSnapshot {
            id: -9,
            gender: 1,
            bytes: vec![0xaa, 0xbb],
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CLEAR_PLAYER_SNAPSHOT, &[7]).unwrap(),
        Some(UiEvent::ClearPlayerSnapshot { id: -9 })
    );
    assert!(parse_ui_event(crate::proto::server::PLAYER_SNAPSHOT, &[7]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::LOBBY_APPEARANCE, &[1, 0xaa]).unwrap(),
        Some(UiEvent::LobbyAppearance {
            gender: 1,
            bytes: vec![0xaa],
        })
    );
    let mut stock = vec![2, 3, 0x0d, 0x01, 0x02];
    stock.extend_from_slice(&enc_p4(101));
    stock.extend_from_slice(&enc_p4(202));
    stock.extend_from_slice(&enc_p4(303));
    stock.extend_from_slice(&enc_p4(404));
    assert_eq!(stock.len(), 21);
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_STOCKMARKET_SLOT, &stock).unwrap(),
        Some(UiEvent::StockmarketSlot {
            market: 2,
            slot: 3,
            state: 0x0d,
            object: 0x0102,
            price: 101,
            count: 202,
            completed_count: 303,
            completed_gold: 404,
        })
    );
    let cutscene = [0x01, 0x02, 0x03, 0x04, 3, 0xaa, 0xbb, 0xcc];
    assert_eq!(
        parse_ui_event(crate::proto::server::CUTSCENE, &cutscene).unwrap(),
        Some(UiEvent::Cutscene {
            id: 0x0102,
            parameter: 0x0304,
            appearance: vec![0xaa, 0xbb, 0xcc],
        })
    );
    // OPENTOP: server encode order (keys, type byte,
    // top) read back in the same order; the lobby window is the lobby top,
    // the game window the world top (`LoginLayout.ts`). 18 bytes is short.
    for (keys, top_id) in [([40, 30, 10, 20], LOBBY_TOP), ([1, 2, 3, 4], GAME_TOP)] {
        let mut top = Vec::new();
        top.extend_from_slice(&enc_p4_alt2(keys[0]));
        top.extend_from_slice(&enc_p4_alt1(keys[1]));
        top.extend_from_slice(&enc_p4_alt2(keys[2]));
        top.extend_from_slice(&enc_p4(keys[3]));
        top.push(0);
        top.extend_from_slice(&enc_p2_alt3(top_id));
        assert_eq!(top.len(), 19);
        match parse_ui_event(crate::proto::server::IF_OPENTOP, &top).unwrap() {
            Some(UiEvent::OpenTop { interface_id, .. }) => {
                assert_eq!(interface_id, top_id as u32)
            }
            other => panic!("expected OpenTop, got {other:?}"),
        }
        assert!(parse_ui_event(crate::proto::server::IF_OPENTOP, &top[..18]).is_err());
    }
    // Non-interface frames return None.
    assert!(parse_ui_event(crate::proto::server::REBUILD_NORMAL, &[])
        .unwrap()
        .is_none());
    assert!(parse_ui_event(crate::proto::server::PLAYER_INFO, &[])
        .unwrap()
        .is_none());
    assert!(parse_ui_event(crate::proto::server::NO_TIMEOUT, &[])
        .unwrap()
        .is_none());
    assert_eq!(
        parse_ui_event(crate::proto::server::CAM_RESET, &[]).unwrap(),
        Some(UiEvent::CameraReset)
    );
    assert!(parse_ui_event(crate::proto::server::CAM_RESET, &[1]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::CAM_SMOOTHRESET, &[]).unwrap(),
        Some(UiEvent::CameraSmoothReset)
    );
    assert!(parse_ui_event(crate::proto::server::CAM_SMOOTHRESET, &[1]).is_err());
    assert_eq!(
        parse_ui_event(
            crate::proto::server::CAM_REMOVEROOF,
            &[0xff, 0xff, 0xff, 0xff]
        )
        .unwrap(),
        Some(UiEvent::CameraRemoveRoof { packed: -1 })
    );
    assert!(parse_ui_event(crate::proto::server::CAM_REMOVEROOF, &[0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_SITESETTINGS, b"settings=foo\0").unwrap(),
        Some(UiEvent::SiteSettings {
            value: "settings=foo".into()
        })
    );
    let uid: [u8; 24] = std::array::from_fn(|index| index as u8 + 1);
    let mut uid_frame = uid.to_vec();
    uid_frame.extend_from_slice(&crc32(&uid).to_be_bytes());
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_UID192, &uid_frame).unwrap(),
        Some(UiEvent::Uid192 { value: Some(uid) })
    );
    uid_frame[27] ^= 1;
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_UID192, &uid_frame).unwrap(),
        Some(UiEvent::Uid192 { value: None })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::LAST_LOGIN_INFO, &[127, 0, 0, 1]).unwrap(),
        Some(UiEvent::LastLoginInfo {
            address: 0x7f00_0001
        })
    );
    let url = [
        vec![1],
        b"javascript:window.open('https://primary')\0".to_vec(),
        b"https://fallback.example/\0".to_vec(),
    ]
    .concat();
    assert_eq!(
        parse_ui_event(crate::proto::server::URL_OPEN, &url).unwrap(),
        Some(UiEvent::UrlOpen {
            primary: "javascript:window.open('https://primary')".into(),
            fallback: Some("https://fallback.example/".into()),
            javascript: true,
        })
    );
    assert_eq!(
        parse_ui_event(
            crate::proto::server::SOCIAL_NETWORK_LOGOUT,
            &[b'h', b't', b't', b'p', b's', b':', b'/', b'/', 0x80],
        )
        .unwrap(),
        Some(UiEvent::SocialNetworkLogout {
            url: "https://€".into()
        })
    );
    // UPDATE_STAT 33/6: g1_alt2, g4_alt1, g1_alt3 order.
    let mut stat = Vec::new();
    stat.push(enc_p1_alt2(99));
    stat.extend_from_slice(&enc_p4_alt1(2_000_000_000));
    stat.push(enc_p1_alt3(7));
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_STAT, &stat).unwrap(),
        Some(UiEvent::Stat {
            current_level: 99,
            xp: 2_000_000_000,
            skill: 7
        })
    );
    assert!(parse_ui_event(crate::proto::server::UPDATE_STAT, &stat[..5]).is_err());
    // SETHIDE parses through the dispatcher.
    let mut hide = Vec::new();
    hide.extend_from_slice(&enc_p4(PLAYER_INFO_SLOT));
    hide.push(enc_p1_alt2(1));
    match parse_ui_event(crate::proto::server::IF_SETHIDE, &hide).unwrap() {
        Some(UiEvent::SetHide { packed, hidden, .. }) => {
            assert_eq!(packed, PLAYER_INFO_SLOT as u32);
            assert!(hidden);
        }
        other => panic!("expected SetHide, got {other:?}"),
    }
    let packed = PLAYER_INFO_SLOT;
    let mut anim = Vec::new();
    anim.extend_from_slice(&enc_p4(packed));
    anim.extend_from_slice(&enc_p4_alt3(1234));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETANIM, &anim).unwrap(),
        Some(UiEvent::SetInterfaceAnim {
            packed: packed as u32,
            animation: 1234
        })
    );
    let mut colour = Vec::new();
    colour.extend_from_slice(&enc_p2_alt2(0x1234));
    colour.extend_from_slice(&enc_p4(packed));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETCOLOUR, &colour).unwrap(),
        Some(UiEvent::SetInterfaceColour {
            packed: packed as u32,
            colour: 0x1234
        })
    );
    let mut angle = Vec::new();
    angle.extend_from_slice(&enc_p4_alt2(packed));
    angle.extend_from_slice(&enc_p2_alt1(101));
    angle.extend_from_slice(&enc_p2_alt1(202));
    angle.extend_from_slice(&enc_p2_alt2(303));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETANGLE, &angle).unwrap(),
        Some(UiEvent::SetInterfaceAngle {
            packed: packed as u32,
            x: 101,
            y: 202,
            zoom: 303
        })
    );
    let mut graphic = Vec::new();
    graphic.extend_from_slice(&enc_p4_alt2(77));
    graphic.extend_from_slice(&enc_p4_alt1(packed));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETGRAPHIC, &graphic).unwrap(),
        Some(UiEvent::SetInterfaceGraphic {
            packed: packed as u32,
            graphic: 77
        })
    );
    let mut model = Vec::new();
    model.extend_from_slice(&enc_p4_alt2(packed));
    model.extend_from_slice(&enc_p4(-123));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETMODEL, &model).unwrap(),
        Some(UiEvent::SetInterfaceModel {
            packed: packed as u32,
            model_kind: 1,
            model: -123,
            model_name_hash: -1,
            local_player: false,
        })
    );
    let mut object = Vec::new();
    object.extend_from_slice(&enc_p2(321));
    object.extend_from_slice(&enc_p4_alt3(packed));
    object.extend_from_slice(&enc_p4_alt1(654321));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETOBJECT, &object).unwrap(),
        Some(UiEvent::SetInterfaceObject {
            packed: packed as u32,
            object: 321,
            count: 654321,
        })
    );
    let mut http_image = Vec::new();
    http_image.extend_from_slice(&enc_p4(packed));
    http_image.extend_from_slice(&enc_p4_alt1(456));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SET_HTTP_IMAGE, &http_image).unwrap(),
        Some(UiEvent::SetInterfaceHttpImage {
            packed: packed as u32,
            image: 456
        })
    );
    let map_flag = [enc_p1_alt1(12), 34];
    assert_eq!(
        parse_ui_event(crate::proto::server::SET_MAP_FLAG, &map_flag).unwrap(),
        Some(UiEvent::SetMapFlag { x: 12, z: 34 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::SET_MAP_FLAG, &[enc_p1_alt1(255), 99]).unwrap(),
        Some(UiEvent::SetMapFlag { x: -1, z: -1 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::MINIMAP_TOGGLE, &[5]).unwrap(),
        Some(UiEvent::MinimapToggle { toggle: 5 })
    );
    let hint_arrow = vec![0x22, 4, 0, 7, 0, 8, 0, 0, 0, 0, 0, 0, 0, 9];
    assert_eq!(
        parse_ui_event(crate::proto::server::HINT_ARROW, &hint_arrow).unwrap(),
        Some(UiEvent::HintArrow { bytes: hint_arrow })
    );
    let hint_trail = vec![3, 0, 42, 66, 0x0c, 0x80, 0x0c, 0x81, 1, 255, 0, 2];
    assert_eq!(
        parse_ui_event(crate::proto::server::HINT_TRAIL, &hint_trail).unwrap(),
        Some(UiEvent::HintTrail {
            slot: 3,
            model: 42,
            points: vec![[3201, 3200], [3201, 3202]],
        })
    );
    let mut player_head = Vec::new();
    player_head.extend_from_slice(&enc_p4_alt1(packed));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETPLAYERHEAD, &player_head).unwrap(),
        Some(UiEvent::SetInterfaceModel {
            packed: packed as u32,
            model_kind: 3,
            model: 0,
            model_name_hash: 0,
            local_player: true,
        })
    );
    let mut player_other = Vec::new();
    player_other.extend_from_slice(&enc_p4_alt1(0x01020304));
    player_other.extend_from_slice(&enc_p2_alt2(4321));
    player_other.extend_from_slice(&enc_p4_alt3(packed));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETPLAYERHEAD_OTHER, &player_other).unwrap(),
        Some(UiEvent::SetInterfaceModel {
            packed: packed as u32,
            model_kind: 3,
            model: 4321,
            model_name_hash: 0x01020304,
            local_player: false,
        })
    );
    let mut snapshot = Vec::new();
    snapshot.extend_from_slice(&enc_p4_alt3(packed));
    snapshot.push(enc_p1_alt1(9));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETPLAYERMODEL_SNAPSHOT, &snapshot).unwrap(),
        Some(UiEvent::SetInterfaceModel {
            packed: packed as u32,
            model_kind: 5,
            model: -11,
            model_name_hash: 0,
            local_player: false,
        })
    );
    let mut npc_head = Vec::new();
    npc_head.extend_from_slice(&enc_p4_alt3(packed));
    npc_head.extend_from_slice(&enc_p4_alt2(0x11223344));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETNPCHEAD, &npc_head).unwrap(),
        Some(UiEvent::SetInterfaceModel {
            packed: packed as u32,
            model_kind: 2,
            model: 0x11223344,
            model_name_hash: -1,
            local_player: false,
        })
    );
    let mut anti = Vec::new();
    anti.extend_from_slice(&enc_p4_alt3(packed));
    anti.push(1);
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETTEXTANTIMACRO, &anti).unwrap(),
        Some(UiEvent::SetInterfaceTextAntiMacro {
            packed: packed as u32,
            enabled: true
        })
    );
    let mut font = Vec::new();
    font.extend_from_slice(&enc_p4_alt1(packed));
    font.extend_from_slice(&enc_p4_alt3(44));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETTEXTFONT, &font).unwrap(),
        Some(UiEvent::SetInterfaceTextFont {
            packed: packed as u32,
            font: 44
        })
    );
    let mut clickmask = Vec::new();
    clickmask.extend_from_slice(&enc_p4_alt3(packed));
    clickmask.push(enc_p1_alt2(1));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETCLICKMASK, &clickmask).unwrap(),
        Some(UiEvent::SetInterfaceClickMask {
            packed: packed as u32,
            enabled: true
        })
    );
    let mut recol = Vec::new();
    recol.extend_from_slice(&enc_p4_alt1(packed));
    recol.extend_from_slice(&enc_p2_alt2(12));
    recol.extend_from_slice(&enc_p2_alt1(34));
    recol.push(enc_p1_alt1(2));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETRECOL, &recol).unwrap(),
        Some(UiEvent::SetInterfaceRecolour {
            packed: packed as u32,
            index: 2,
            source: 12,
            destination: 34
        })
    );
    let mut retex = Vec::new();
    retex.extend_from_slice(&enc_p2_alt3(56));
    retex.extend_from_slice(&enc_p2_alt1(78));
    retex.extend_from_slice(&enc_p4(packed));
    retex.push(enc_p1_alt1(3));
    assert_eq!(
        parse_ui_event(crate::proto::server::IF_SETRETEX, &retex).unwrap(),
        Some(UiEvent::SetInterfaceRetexture {
            packed: packed as u32,
            index: 3,
            source: 56,
            destination: 78
        })
    );
}

#[test]
fn parse_client_varcs_uses_wire_orders_and_signed_values() {
    let small = [0x82, 0x12, 0x34];
    assert_eq!(
        parse_ui_event(crate::proto::server::CLIENT_SETVARC_SMALL, &small).unwrap(),
        Some(UiEvent::SetVarc {
            id: 0x1234,
            value: -2
        })
    );
    let large = [0xFB, 0xFF, 0xFF, 0xFF, 0x01, 0x81];
    assert_eq!(
        parse_ui_event(crate::proto::server::CLIENT_SETVARC_LARGE, &large).unwrap(),
        Some(UiEvent::SetVarc {
            id: 0x0101,
            value: -5
        })
    );
    let bit_small = [0x12, 0x34, 0x82];
    assert_eq!(
        parse_ui_event(crate::proto::server::CLIENT_SETVARCBIT_SMALL, &bit_small).unwrap(),
        Some(UiEvent::SetVarcBit {
            id: 0x1234,
            value: -2
        })
    );
    let bit_large = [0x01, 0x81, 0xFB, 0xFF, 0xFF, 0xFF];
    assert_eq!(
        parse_ui_event(crate::proto::server::CLIENT_SETVARCBIT_LARGE, &bit_large).unwrap(),
        Some(UiEvent::SetVarcBit {
            id: 0x0101,
            value: -5
        })
    );
    let string = [0x00, 0x2A, b'h', b'i', 0];
    assert_eq!(
        parse_ui_event(crate::proto::server::CLIENT_SETVARCSTR_SMALL, &string).unwrap(),
        Some(UiEvent::SetVarcString {
            id: 42,
            value: "hi".encode_utf16().collect()
        })
    );
    assert!(parse_ui_event(crate::proto::server::CLIENT_SETVARC_LARGE, &large[..5]).is_err());
}

#[test]
fn drain_pending_sync_collects_ui_queues_replies_and_no_blocks() {
    use crate::net::{decode_frame, ResyncState};
    // Buffered IF_OPENTOP (lobby window) + REBUILD_NORMAL: the sync drain must
    // surface the rebuild, collect the UI event, and queue
    // MAP_BUILD_COMPLETE without touching a runtime.
    let mut top = Vec::new();
    top.extend_from_slice(&enc_p4_alt2(40));
    top.extend_from_slice(&enc_p4_alt1(30));
    top.extend_from_slice(&enc_p4_alt2(10));
    top.extend_from_slice(&enc_p4(20));
    top.push(0);
    top.extend_from_slice(&enc_p2_alt3(LOBBY_TOP));
    let tail = server_rebuild_tail(402, 402, 5, 9, 0, true);
    let mut pending = Vec::new();
    // Fixed-size IF_OPENTOP (19): opcode + payload, no length prefix.
    pending.push(crate::proto::server::IF_OPENTOP);
    pending.extend_from_slice(&top);
    // Variable REBUILD_NORMAL (-2): opcode + u16 BE len + payload.
    pending.push(crate::proto::server::REBUILD_NORMAL);
    pending.extend_from_slice(&(tail.len() as u16).to_be_bytes());
    pending.extend_from_slice(&tail);
    // Sanity: the strict decoder sees the first frame (framing check).
    assert!(decode_frame(&pending).unwrap().is_some());
    let mut state = ResyncState::default();
    let mut writes = Vec::new();
    let mut ui_out = Vec::new();
    let event = drain_pending_sync(&mut pending, &mut state, &mut writes, &mut ui_out)
        .unwrap()
        .expect("buffered rebuild must surface");
    assert_eq!((event.rebuild.zone_x, event.rebuild.zone_z), (402, 402));
    assert_eq!(ui_out.len(), 1);
    match &ui_out[0] {
        UiEvent::OpenTop { interface_id, .. } => assert_eq!(*interface_id, u32::from(LOBBY_TOP)),
        other => panic!("expected OpenTop, got {other:?}"),
    }
    assert_eq!(writes.len(), 5);
    assert_eq!(writes[0], crate::proto::client::MAP_BUILD_COMPLETE);
    assert!(pending.is_empty());
    // No-block: empty pending returns immediately with no event.
    let mut state = ResyncState::default();
    let mut writes = Vec::new();
    let mut ui_out = Vec::new();
    let mut empty: Vec<u8> = Vec::new();
    let none = drain_pending_sync(&mut empty, &mut state, &mut writes, &mut ui_out).unwrap();
    assert!(none.is_none());
    assert!(ui_out.is_empty());
    assert!(writes.is_empty());
}

#[test]
fn fragmented_stat_frames_preserve_live_dispatch_order() {
    // Full frames emitted by the TypeScript UPDATE_STAT encoder, including
    // little-endian signed XP and a partial second frame at each boundary.
    let frames = [33, 136, 120, 86, 52, 18, 113, 33, 0, 0, 0, 0, 128, 102];
    for split in 0..=frames.len() {
        let mut pending = frames[..split].to_vec();
        let mut state = crate::net::ResyncState::default();
        let mut writes = Vec::new();
        let mut events = Vec::new();
        drain_pending_sync(&mut pending, &mut state, &mut writes, &mut events).unwrap();
        assert_eq!(events.len(), split / 7);
        pending.extend_from_slice(&frames[split..]);
        drain_pending_sync(&mut pending, &mut state, &mut writes, &mut events).unwrap();
        assert_eq!(
            events,
            vec![
                UiEvent::Stat {
                    skill: 15,
                    current_level: 120,
                    xp: 0x12345678
                },
                UiEvent::Stat {
                    skill: 26,
                    current_level: 0,
                    xp: i32::MIN
                },
            ]
        );
        assert!(pending.is_empty());
        assert!(writes.is_empty());
    }
}

#[test]
fn navigation_control_packets_follow_their_decoders() {
    for value in [0u8, 100, 255] {
        assert_eq!(
            parse_ui_event(crate::proto::server::UPDATE_RUNENERGY, &[value]).unwrap(),
            Some(UiEvent::RunEnergy { value })
        );
    }
    for value in [i16::MIN, -1, 32767] {
        assert_eq!(
            parse_ui_event(crate::proto::server::UPDATE_RUNWEIGHT, &value.to_be_bytes()).unwrap(),
            Some(UiEvent::RunWeight { value })
        );
    }
    assert!(parse_ui_event(crate::proto::server::UPDATE_RUNENERGY, &[]).is_err());
    assert!(parse_ui_event(crate::proto::server::UPDATE_RUNWEIGHT, &[0]).is_err());
    assert_eq!(
        parse_ui_event(
            crate::proto::server::UPDATE_REBOOT_TIMER,
            &1200u16.to_be_bytes()
        )
        .unwrap(),
        Some(UiEvent::RebootTimer { ticks: 1200 })
    );
    assert!(parse_ui_event(crate::proto::server::UPDATE_REBOOT_TIMER, &[0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::SET_TARGET, &[0xfb, 0xae]).unwrap(),
        Some(UiEvent::SetTarget { value: -1234 })
    );
    assert!(parse_ui_event(crate::proto::server::SET_TARGET, &[0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::SETDRAWORDER, &[127]).unwrap(),
        Some(UiEvent::SetDrawOrder { value: 1 })
    );
    assert!(parse_ui_event(crate::proto::server::SETDRAWORDER, &[]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::CHAT_FILTER_SETTINGS, &[254, 255]).unwrap(),
        Some(UiEvent::ChatFilters {
            trade: 2,
            public: 1
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CHAT_FILTER_SETTINGS_PRIVATECHAT, &[2]).unwrap(),
        Some(UiEvent::ChatPrivateFilter { value: Some(2) })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CHAT_FILTER_SETTINGS_PRIVATECHAT, &[3]).unwrap(),
        Some(UiEvent::ChatPrivateFilter { value: None })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_CHECK_EMAIL_REPLY, &[21]).unwrap(),
        Some(UiEvent::CreateEmailReply { value: 21 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_CHECK_EMAIL_REPLY, &[0xff]).unwrap(),
        Some(UiEvent::CreateEmailReply { value: 3 })
    );
    assert!(parse_ui_event(crate::proto::server::CREATE_CHECK_EMAIL_REPLY, &[]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_ACCOUNT_REPLY, &[38]).unwrap(),
        Some(UiEvent::AccountCreationResult { value: 38 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_CHECK_NAME_REPLY, &[8]).unwrap(),
        Some(UiEvent::CreateNameReply { value: 8 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_SUGGEST_NAME_ERROR, &[0xff]).unwrap(),
        Some(UiEvent::CreateSuggestNameError { value: 3 })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::CREATE_SUGGEST_NAME_REPLY, b"Rune\0").unwrap(),
        Some(UiEvent::CreateSuggestName {
            value: "Rune".into()
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::UPDATE_DOB, &[0xff, 0xfb, 0x2e, 1]).unwrap(),
        Some(UiEvent::UpdateDob {
            dob: -1234,
            verified: true
        })
    );
    assert!(parse_ui_event(crate::proto::server::UPDATE_DOB, &[0, 0, 0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::LOYALTY_UPDATE, &[2, 1, 4, 3]).unwrap(),
        Some(UiEvent::LoyaltyUpdate { value: 0x0102_0304 })
    );
    assert!(parse_ui_event(crate::proto::server::LOYALTY_UPDATE, &[0, 0, 0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::JCOINS_UPDATE, &[3, 4, 1, 2]).unwrap(),
        Some(UiEvent::JCoinsUpdate { value: 0x0102_0304 })
    );
    assert!(parse_ui_event(crate::proto::server::JCOINS_UPDATE, &[0, 0, 0]).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::TRIGGER_ONDIALOGABORT, &[]).unwrap(),
        Some(UiEvent::TriggerDialogAbort)
    );
    assert!(parse_ui_event(crate::proto::server::TRIGGER_ONDIALOGABORT, &[0]).is_err());
    assert_eq!(
        parse_ui_event(
            crate::proto::server::MESSAGE_GAME,
            &[1, 0, 0, 0, 2, 0, b'h', b'i', 0]
        )
        .unwrap(),
        Some(UiEvent::GameMessage {
            chat_type: 1,
            flags: 2,
            name: String::new(),
            name_unfiltered: String::new(),
            message: "hi".into()
        })
    );
    let mut private = vec![0, b'B', b'o', b'b', 0, 0, 1, 0, 0, 7, 1, 2, 0x50];
    assert_eq!(
        parse_ui_event(crate::proto::server::MESSAGE_PRIVATE, &private).unwrap(),
        Some(UiEvent::PrivateMessage {
            bytes: std::mem::take(&mut private)
        })
    );
    assert!(parse_ui_event(crate::proto::server::MESSAGE_PRIVATE, &[0, b'B']).is_err());
    assert_eq!(
        parse_ui_event(crate::proto::server::LOGOUT, &[7]).unwrap(),
        Some(UiEvent::Logout {
            reason: 7,
            full: false
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::LOGOUT_FULL, &[8]).unwrap(),
        Some(UiEvent::Logout {
            reason: 8,
            full: true
        })
    );
    let lobby = [b'l', b'o', b'b', b'b', b'y', 0, 0, 9, 0, 10, 0, 11];
    assert_eq!(
        parse_ui_event(crate::proto::server::CHANGE_LOBBY, &lobby).unwrap(),
        Some(UiEvent::ChangeLobby {
            host: "lobby".into(),
            node: 9,
            port: 10,
            port2: 11
        })
    );
    let transfer = [0, 42, b'w', b'o', b'r', b'l', b'd', 0, 0, 20, 0, 21, 1];
    assert_eq!(
        parse_ui_event(crate::proto::server::LOGOUT_TRANSFER, &transfer).unwrap(),
        Some(UiEvent::LogoutTransfer {
            world_id: 42,
            host: "world".into(),
            port: 20,
            port2: 21,
            cancellable: true
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::SET_MOVEACTION, &[]).unwrap(),
        Some(UiEvent::SetMoveAction {
            text: "Walk here".into(),
            action: -1
        })
    );
    assert_eq!(
        parse_ui_event(
            crate::proto::server::SET_MOVEACTION,
            &[b'G', b'o', 0, 0xff, 0xff]
        )
        .unwrap(),
        Some(UiEvent::SetMoveAction {
            text: "Go".into(),
            action: -1
        })
    );
    assert_eq!(
        parse_ui_event(crate::proto::server::SHOW_FACE_HERE, &[127]).unwrap(),
        Some(UiEvent::ShowFaceHere { enabled: true })
    );
    let op = [254, 128, 0, b'F', b'o', b'l', b'l', b'o', b'w', 0, 128];
    assert_eq!(
        parse_ui_event(crate::proto::server::SET_PLAYER_OP, &op).unwrap(),
        Some(UiEvent::SetPlayerOp {
            slot: 2,
            cursor: 0,
            name: Some("Follow".into()),
            deprioritised: true
        })
    );
    assert_eq!(
        parse_ui_event(
            crate::proto::server::SET_PLAYER_OP,
            &[0, 128, 0, b'n', 0, 128]
        )
        .unwrap(),
        None
    );
    assert_eq!(
        parse_ui_event(
            crate::proto::server::SET_PLAYER_OP,
            &[247, 128, 0, b'n', 0, 128]
        )
        .unwrap(),
        None
    );
    assert!(parse_ui_event(crate::proto::server::SHOW_FACE_HERE, &[]).is_err());
}

/// Pack-free field orders of the sub, camera-shake and attack-priority
/// packets (the live E2E, client910
/// `app::scenario_tests::live_server_packets_reach_their_consumers`, needs
/// the pack): IF_CLOSESUB `g4_alt2`, IF_MOVESUB
/// `g4_alt2` source then target, REDUCE_PLAYER_ATTACK_PRIORITY
/// `g1`, CAM_SHAKE `g2_alt2 g1_alt1 g1_alt2 g1_alt3 g1`. `p4_alt2` writes
/// `v >> 8, v, v >> 24, v >> 16`.
#[test]
fn sub_camera_and_priority_packets_decode_field_order() {
    use crate::proto::server as p;
    let source = [0x56, 0x78, 0x12, 0x34]; // p4_alt2(0x1234_5678)
    let target = [0xde, 0xf0, 0x9a, 0xbc]; // p4_alt2(0x9abc_def0)
    assert_eq!(
        parse_ui_event(p::IF_CLOSESUB, &source).unwrap(),
        Some(UiEvent::CloseSub {
            parent_packed: 0x1234_5678
        })
    );
    assert_eq!(
        parse_ui_event(p::IF_MOVESUB, &[source, target].concat()).unwrap(),
        Some(UiEvent::MoveSub {
            source_packed: 0x1234_5678,
            target_packed: 0x9abc_def0
        })
    );
    assert_eq!(
        parse_ui_event(p::REDUCE_PLAYER_ATTACK_PRIORITY, &[3]).unwrap(),
        Some(UiEvent::PlayerAttackPriority { value: 3 })
    );
    // p2_alt2(300) = [1, 300 + 128], p1_alt1(2) = 130, p1_alt2(10) = -10,
    // p1_alt3(20) = 108, p1(30).
    assert_eq!(
        parse_ui_event(p::CAM_SHAKE, &[1, 172, 130, 246, 108, 30]).unwrap(),
        Some(UiEvent::CameraShake {
            channel: 2,
            jitter: 10,
            wobble_scale: 20,
            cycle: 30,
            wobble_speed: 300
        })
    );
}

/// Point-light packet field order: colour is the
/// leading `g4_alt3`; the intensity packet's second `g2_alt2` is the
/// group and the trailing `g1` the percentage (255 -> -1). Decoded
/// through the live render-loop handler, which queues no reply.
#[test]
fn point_light_packets_decode_field_order() {
    let live = |opcode, payload: &[u8]| {
        let (mut writes, mut ui_out) = (Vec::new(), Vec::new());
        let frame = crate::net::Frame {
            opcode,
            payload: payload.to_vec(),
        };
        assert!(handle_sync_frame(frame, &mut writes, &mut ui_out)
            .unwrap()
            .is_none());
        assert!(writes.is_empty());
        ui_out
    };
    assert_eq!(
        live(
            crate::proto::server::POINTLIGHT_COLOUR,
            &[1, 2, 3, 4, 5, 6, 7, 8]
        ),
        [UiEvent::PointLightColour {
            duration: CoreReader::new(&[5, 6]).g2_alt1().unwrap(),
            colour: CoreReader::new(&[1, 2, 3, 4]).g4_alt3().unwrap(),
            id: CoreReader::new(&[7, 8]).g2_alt3().unwrap(),
        }]
    );
    for (raw, intensity) in [(255, -1), (40, 40)] {
        assert_eq!(
            live(
                crate::proto::server::POINTLIGHT_INTENSITY,
                &[1, 2, 3, 4, raw]
            ),
            [UiEvent::PointLightIntensity {
                duration: CoreReader::new(&[1, 2]).g2_alt2().unwrap(),
                intensity,
                id: CoreReader::new(&[3, 4]).g2_alt2().unwrap(),
            }]
        );
    }
}

/// What arrives before the first rebuild is read like anything else: the
/// startup drain keeps the site settings, the uid, the last login, the URL,
/// the player snapshots, the lobby appearance, the stock market slot and the
/// cutscene, in their place among the other frames, to apply once the client
/// has its window.
#[test]
fn startup_drain_keeps_every_interface_frame() {
    use crate::proto::server as sp;
    let uid = [7u8; 24];
    let mut uid_frame = uid.to_vec();
    uid_frame.extend(rs910_core::checksum::crc32(&uid).to_be_bytes());
    let frames: Vec<(u8, Vec<u8>)> = vec![
        (sp::UPDATE_SITESETTINGS, b"site\0".to_vec()),
        (sp::UPDATE_UID192, uid_frame),
        (sp::LAST_LOGIN_INFO, vec![127, 0, 0, 1]),
        (
            sp::URL_OPEN,
            [&[0u8][..], b"https://example.com\0"].concat(),
        ),
        (sp::PLAYER_SNAPSHOT, vec![1, 0]),
        (sp::CLEAR_PLAYER_SNAPSHOT, vec![1]),
        (sp::LOBBY_APPEARANCE, vec![0]),
        (sp::UPDATE_STOCKMARKET_SLOT, vec![0; 21]),
        (sp::CUTSCENE, vec![0, 2, 0, 64, 0]),
    ];
    let mut drain = StartupDrain::new(false);
    for (opcode, payload) in &frames {
        let frame = crate::net::Frame {
            opcode: *opcode,
            payload: payload.clone(),
        };
        assert_eq!(drain.on_frame(frame, || 0).unwrap(), None);
    }
    let kinds: Vec<_> = drain.outcome.ui_events.iter().map(event_kind).collect();
    assert_eq!(
        kinds,
        [
            "SiteSettings",
            "Uid192",
            "LastLoginInfo",
            "UrlOpen",
            "PlayerSnapshot",
            "ClearPlayerSnapshot",
            "LobbyAppearance",
            "StockmarketSlot",
            "Cutscene"
        ]
    );
}

/// A known packet whose payload does not decode becomes a malformed-packet
/// event in its place (the read logs the player out when it reaches it), in
/// the startup drain and the live poll alike; frames around it are kept.
#[test]
fn a_payload_that_does_not_decode_is_a_malformed_packet() {
    use crate::proto::server as sp;
    let truncated = crate::net::Frame {
        opcode: sp::IF_SETTEXT,
        payload: vec![0, 1],
    };
    let good = crate::net::Frame {
        opcode: sp::LAST_LOGIN_INFO,
        payload: vec![127, 0, 0, 1],
    };
    let mut drain = StartupDrain::new(false);
    for frame in [good.clone(), truncated.clone(), good.clone()] {
        drain.on_frame(frame, || 0).unwrap();
    }
    let kinds: Vec<_> = drain.outcome.ui_events.iter().map(event_kind).collect();
    assert_eq!(kinds, ["LastLoginInfo", "MalformedPacket", "LastLoginInfo"]);
    assert!(matches!(
        drain.outcome.ui_events[1],
        UiEvent::MalformedPacket {
            opcode: sp::IF_SETTEXT,
            size: 2,
            ..
        }
    ));
    let mut ui_out = Vec::new();
    let mut writes = Vec::new();
    handle_sync_frame(truncated, &mut writes, &mut ui_out).unwrap();
    handle_sync_frame(good, &mut writes, &mut ui_out).unwrap();
    let kinds: Vec<_> = ui_out.iter().map(event_kind).collect();
    assert_eq!(kinds, ["MalformedPacket", "LastLoginInfo"]);
    assert!(writes.is_empty(), "nothing is answered");
}

// -- IO seam (lane Q-SESSION) --------------------------------------------

#[test]
fn startup_drain_dispatch_needs_no_stream_or_clock() {
    let frame = |opcode: u8, payload: Vec<u8>| crate::net::Frame { opcode, payload };
    let tail = server_rebuild_tail(402, 402, 5, 9, 0, true);
    let mut drain = StartupDrain::new(false);
    let no_clock = || -> i32 { panic!("only MAP_BUILD_COMPLETE samples the clock") };
    assert_eq!(
        drain
            .on_frame(frame(crate::proto::server::NO_TIMEOUT, vec![]), no_clock)
            .unwrap(),
        None,
        "the server's keepalive is not answered"
    );
    let reply = drain
        .on_frame(
            frame(crate::proto::server::REBUILD_NORMAL, tail.to_vec()),
            || 7,
        )
        .unwrap();
    assert_eq!(reply, Some(StartupReply::MapBuildComplete(7)));
    assert_eq!(
        reply.unwrap().bytes(),
        crate::net::encode_map_build_complete(7)
    );
    assert!(!drain.finished);
    assert_eq!(
        drain
            .on_frame(frame(crate::proto::server::PLAYER_INFO, vec![]), no_clock)
            .unwrap(),
        None
    );
    assert!(!drain.finished, "an empty PLAYER_INFO waits for the next");
    drain
        .on_frame(
            frame(crate::proto::server::PLAYER_INFO, vec![0xAA; 64]),
            no_clock,
        )
        .unwrap();
    assert!(drain.finished);
    assert_eq!(drain.outcome.player_info_bytes, 64);
    assert_eq!(
        drain.outcome.rebuild.as_ref().map(|r| (r.zone_x, r.zone_z)),
        Some((402, 402))
    );
    // Strict entities: the rebuild commits first, nothing is sent.
    let mut strict = StartupDrain::new(true);
    assert_eq!(
        strict
            .on_frame(
                frame(crate::proto::server::REBUILD_NORMAL, tail.to_vec()),
                no_clock
            )
            .unwrap(),
        None
    );
    assert!(strict.finished);
}

/// A non-blocking socket double: `write` takes at most `chunk` bytes, then
/// `WouldBlock` after `budget` calls; `read` hands out `input` in `chunk`s,
/// then `WouldBlock` (or EOF when `eof`).
struct Nonblocking {
    chunk: usize,
    budget: usize,
    written: Vec<u8>,
    input: Vec<u8>,
    eof: bool,
}

impl std::io::Write for Nonblocking {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.budget == 0 {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        self.budget -= 1;
        let n = buf.len().min(self.chunk);
        self.written.extend_from_slice(&buf[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl std::io::Read for Nonblocking {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.input.is_empty() {
            return if self.eof {
                Ok(0)
            } else {
                Err(std::io::ErrorKind::WouldBlock.into())
            };
        }
        let n = self.input.len().min(self.chunk).min(buf.len());
        buf[..n].copy_from_slice(&self.input[..n]);
        self.input.drain(..n);
        Ok(n)
    }
}

#[test]
fn nonblocking_transport_keeps_unsent_bytes_and_reports_close() {
    let mut socket = Nonblocking {
        chunk: 3,
        budget: 2,
        written: vec![],
        input: (0..10).collect(),
        eof: false,
    };
    let mut queue: Vec<u8> = (100..108).collect();
    let mut seen = Vec::new();
    flush_nonblocking(&mut socket, &mut queue, |chunk| seen.push(chunk.to_vec())).unwrap();
    assert_eq!(socket.written, [100, 101, 102, 103, 104, 105]);
    assert_eq!(seen, [vec![100, 101, 102], vec![103, 104, 105]]);
    assert_eq!(queue, [106, 107], "WouldBlock keeps the tail queued");
    let mut pending = vec![9];
    let mut reads = 0;
    let closed = read_nonblocking(&mut socket, &mut pending, |_| reads += 1).unwrap();
    assert!(!closed);
    assert_eq!(pending, [9, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    assert_eq!(reads, 4);
    socket.input = vec![1, 2];
    socket.eof = true;
    assert!(read_nonblocking(&mut socket, &mut pending, |_| {}).unwrap());
    assert_eq!(
        &pending[pending.len() - 2..],
        [1, 2],
        "bytes before EOF stay"
    );
    socket.budget = 5;
    socket.chunk = 0;
    flush_nonblocking(&mut socket, &mut queue, |_| panic!("zero write")).unwrap();
    assert_eq!(queue, [106, 107], "a zero write stops the flush");
}
