#[test]
fn socket_receives_later_packets_after_startup_reactor_is_dropped() -> anyhow::Result<()> {
    use std::io::{ErrorKind, Read, Write};
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let stream = runtime.block_on(tokio::net::TcpStream::connect(listener.local_addr()?))?;
    let (mut peer, _) = listener.accept()?;
    let mut stream = super::detach_world_socket(crate::wire_stream::WireStream::plain(stream))?;
    let mut bytes = [0; 32];
    assert_eq!(
        stream.read(&mut bytes).unwrap_err().kind(),
        ErrorKind::WouldBlock
    );
    drop(runtime);
    peer.write_all(b"later packet")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    let mut got = Vec::new();
    while got.len() < 12 {
        match stream.read(&mut bytes) {
            Ok(0) => anyhow::bail!("socket closed before packet"),
            Ok(n) => got.extend_from_slice(&bytes[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => {
                anyhow::ensure!(
                    std::time::Instant::now() < deadline,
                    "later packet never became readable"
                );
                std::thread::yield_now();
            }
            Err(e) => return Err(e.into()),
        }
    }
    assert_eq!(got, b"later packet");
    Ok(())
}
use super::*;

fn rebuild(zone_x: u16, zone_z: u16) -> crate::session::Rebuild {
    crate::session::Rebuild {
        zone_x,
        zone_z,
        npc_bits: 5,
        map_count: 9,
        build_area_id: 0,
        force: true,
        has_high_res_block: false,
        high_res_bytes: 0,
    }
}

#[test]
fn spawn_for_rebuild_targets_zone_centre() {
    // Lumbridge spawn zone (3222 >> 3 = 402) centres on (3220, 3220):
    // within two tiles of the hardcoded spawn, so the spawn path stays
    // visually identical.
    assert_eq!(spawn_for_rebuild(&rebuild(402, 402)), (3220, 3220));
    assert_eq!(spawn_for_rebuild(&rebuild(300, 310)), (2404, 2484));
}

/// Queued reflection checks are answered only in state 18; other
/// connections drop them (preparing for a map discards them).
#[test]
fn reflection_checks_answer_only_in_game() {
    let check = crate::reflection_check::Check {
        id: 9,
        entries: vec![crate::reflection_check::Entry { op: 0, status: -1 }],
    };
    let events = vec![
        crate::session::UiEvent::ReflectionProbe(check.clone()),
        crate::session::UiEvent::Js5Reload,
    ];
    let mut in_game = events.clone();
    let mut writes = Vec::new();
    answer_reflection_checks(&mut in_game, true, &mut writes);
    assert_eq!(writes, crate::reflection_check::encode_reply(&check));
    assert_eq!(in_game, [crate::session::UiEvent::Js5Reload]);
    let mut lobby = events;
    let mut writes = Vec::new();
    answer_reflection_checks(&mut lobby, false, &mut writes);
    assert!(writes.is_empty());
    assert_eq!(lobby, [crate::session::UiEvent::Js5Reload]);
    assert!(ends_connection(&crate::session::UiEvent::Js5Reload));
    assert!(ends_connection(&crate::session::UiEvent::UnhandledPacket {
        opcode: 51,
        size: 0
    }));
    assert!(is_session_event(&crate::session::UiEvent::DoCheat {
        command: "x".into()
    }));
    assert!(!is_client_debug_event(
        &crate::session::UiEvent::DebugServerTriggers {
            interface: 1,
            start: 0,
            end: 0,
            values: vec![],
        }
    ));
}

#[test]
fn sound_area_listener_filter_matches_the_range_gate() {
    let mut player = crate::entities910::Player {
        level: 1,
        ..Default::default()
    };
    player.x[0] = 100;
    player.z[0] = 200;
    let sound = crate::protocol910::zone_state::SoundArea {
        level: 1,
        x: 103,
        z: 197,
        sound: 42,
        loops: 0,
        radius: 2,
        delay: 0,
        volume: 255,
        rate: 256,
        dialog: false,
    };
    assert!(sound_area_audible(
        Some((player.level, player.x[0], player.z[0])),
        crate::protocol910::rebuild_state::Kind::Normal,
        &sound
    ));
    let mut far = sound.clone();
    far.x = 104;
    assert!(!sound_area_audible(
        Some((player.level, player.x[0], player.z[0])),
        crate::protocol910::rebuild_state::Kind::Normal,
        &far
    ));
    assert!(!sound_area_audible(
        Some((player.level, player.x[0], player.z[0])),
        crate::protocol910::rebuild_state::Kind::Cutscene,
        &sound
    ));
    // The zone level is never compared and sound -1 is not filtered: both
    // go on to the audio API.
    let other_level = crate::protocol910::zone_state::SoundArea {
        level: 2,
        sound: -1,
        ..sound.clone()
    };
    assert!(sound_area_audible(
        Some((player.level, player.x[0], player.z[0])),
        crate::protocol910::rebuild_state::Kind::Normal,
        &other_level
    ));
}

// --- Live client state over the real cache ---

/// Logout resets the transmit counters: a relog starts every transmit counter at zero while the client variable
/// values themselves survive.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn logout_resets_transmit_nums_but_keeps_client_variables() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    let mut game = crate::client_game::ClientGame::login(
        &pack,
        1,
        crate::protocol910::live::Feed::default(),
        910,
        false,
    )?;
    game.runtime.varp_transmit_num = 9;
    let vars = &mut game.ui_variables;
    vars.varc_transmit.push(11);
    vars.string_transmit.push(12);
    vars.inv_transmit.push(93);
    vars.stat_transmit.push(3);
    vars.varclan_transmit.push(5);
    vars.client
        .values
        .insert(1234, crate::ui_vars::Value::Int(77));
    let next = logged_out_game(&pack, Some(game))?;
    let vars = &next.ui_variables;
    assert_eq!(next.runtime.varp_transmit_num, 0);
    for ring in [
        &vars.varc_transmit,
        &vars.string_transmit,
        &vars.inv_transmit,
        &vars.stat_transmit,
        &vars.varclan_transmit,
    ] {
        assert_eq!(ring.count, 0);
    }
    assert_eq!(
        vars.client.values.get(&1234),
        Some(&crate::ui_vars::Value::Int(77))
    );
    let mut cycles = crate::ui_loop::Cycles {
        redraw: 40,
        chat: 1,
        friend: 2,
        clan: 3,
        clan_settings: 4,
        clan_channel: 5,
        stock: 6,
        misc: 7,
        player_group: 8,
        player_group_varp: 9,
        ..Default::default()
    };
    cycles.reset_transmit_nums();
    assert_eq!(cycles.redraw, 40);
    assert_eq!(
        [
            cycles.chat,
            cycles.friend,
            cycles.clan,
            cycles.clan_settings,
            cycles.clan_channel,
            cycles.stock,
            cycles.misc,
            cycles.player_group,
            cycles.player_group_varp
        ],
        [0; 9]
    );
    Ok(())
}

/// A world change (hop or reconnect from the save) keeps the variable
/// transmit counter counting: the interface's components remember the last
/// transmit they saw, and a counter restarted at zero would leave the new
/// world's variables behind them (the hopped-to world showed the layout as
/// unlocked). The client variables come along too.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn a_world_change_keeps_the_transmit_counter_and_client_variables() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    let login = |local| {
        crate::client_game::ClientGame::login(
            &pack,
            local,
            crate::protocol910::live::Feed::default(),
            910,
            false,
        )
    };
    let mut previous = login(1)?;
    previous.runtime.varp_transmit_num = 41;
    previous.runtime.varp_transmitted[40] = 3814;
    previous
        .ui_variables
        .client
        .values
        .insert(1234, crate::ui_vars::Value::Int(77));
    let mut next = login(2)?;
    assert!(adopt_client_variables(Some(previous), &mut next, false)?);
    assert_eq!(next.runtime.varp_transmit_num, 41);
    assert_eq!(next.runtime.varp_transmitted[40], 3814);
    assert_eq!(
        next.ui_variables.client.values.get(&1234),
        Some(&crate::ui_vars::Value::Int(77))
    );
    Ok(())
}

/// Showing the login and lobby screens over the real 910 cache: the title
/// opens the defaults' `login_interface` (744), the lobby server's 906 tree
/// is replaced by it again on logout, and showing the lobby with
/// lobby_interface -1 leaves no top level for the lobby IF_OPENTOP.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn title_and_lobby_trees_replace_through_show_top_level() -> anyhow::Result<()> {
    use rs910_symbols::{component, interface};
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    assert_eq!(title_interfaces(&pack)?, (interface::LOGIN_SCREEN.id(), -1));
    let mut game = title_game(&pack)?;
    let mut ui = crate::ui_runtime::Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    crate::client_game::with_game(&mut game, |v| {
        ui.show_top_level(v, interface::LOGIN_SCREEN.id())
    })?;
    assert_eq!(ui.state.life.top, interface::LOGIN_SCREEN.id());
    // LoginLayout.ts (lobby): the top 906, then 907 in 906:107.
    let lobby_sub = component::lobby_window::PLAYER_INFO_SLOT.packed();
    crate::client_game::with_game(&mut game, |v| {
        ui.packet(
            v,
            &crate::session::UiEvent::OpenTop {
                interface_id: interface::LOBBY_WINDOW.id() as u32,
                keys: [0; 4],
            },
        )?;
        ui.packet(
            v,
            &crate::session::UiEvent::OpenSub {
                parent_packed: lobby_sub as u32,
                sub_id: interface::LOBBY_PLAYER_INFO.id() as u32,
                kind: 1,
                keys: [0; 4],
            },
        )
    })?;
    assert_eq!(ui.state.life.top, interface::LOBBY_WINDOW.id());
    assert!(ui.state.life.subs.get(lobby_sub).is_some());
    // logout(false) -> state 4 -> the login screen (top != 744).
    crate::client_game::with_game(&mut game, |v| {
        ui.show_top_level(v, interface::LOGIN_SCREEN.id())
    })?;
    assert_eq!(ui.state.life.top, interface::LOGIN_SCREEN.id());
    assert!(ui.state.life.subs.get(lobby_sub).is_none());
    assert!(!ui
        .store
        .interfaces
        .contains_key(&interface::LOBBY_WINDOW.id()));
    // Lobby reply -> setState(13) -> showLobby(true) with lobby_interface -1.
    crate::client_game::with_game(&mut game, |v| ui.show_top_level(v, -1))?;
    assert_eq!(ui.state.life.top, -1);
    assert!(ui.store.interfaces.is_empty());
    assert_eq!(ui.diagnostics.failures, 0);
    Ok(())
}

/// Phase 4.4: the engine's requests after the interface tick drain in the
/// order `about_to_wait` applied the old `SessionRequests` fields (script
/// URLs, scripts, world switch, cancel, logout, login, lobby enter game, and
/// the account-creation connect after the title world).
#[test]
fn session_requests_keep_the_old_drain_order() {
    use crate::ui_runtime::host_builtins::Request;
    let mut engine = crate::ui_runtime::Engine::default();
    engine.login.world_switch = Some(crate::ui_runtime::WorldSwitchRequest {
        world_id: 2,
        host: "h".into(),
    });
    engine.login.request = Some(crate::ui_runtime::LoginRequest {
        username: "u".into(),
        password: "p".into(),
        new_auth_preference: String::new(),
        auth_dont_trust: false,
        lobby: true,
        sso: None,
    });
    engine.login.lobby_enter_game = Some(crate::ui_runtime::LobbyEnterGameRequest {
        new_auth_preference: String::new(),
        auth_dont_trust: false,
    });
    engine.creation.connect_requested = true;
    engine.login.cancel_requested = true;
    engine.login.logout_requested = true;
    engine.builtins.requests = vec![Request::Cheat("a".into()), Request::Quit];
    engine.effects.browser_urls = vec![crate::session::UiEvent::SocialNetworkLogout {
        url: "https://a".into(),
    }];
    let effects = session_requests(&mut engine);
    let kinds: Vec<&str> = effects
        .iter()
        .map(|e| match e {
            ClientEffect::ScriptUrl(_) => "url",
            ClientEffect::Script(Request::Cheat(_)) => "cheat",
            ClientEffect::Script(Request::Quit) => "quit",
            ClientEffect::WorldSwitch(_) => "switch",
            ClientEffect::LoginCancel => "cancel",
            ClientEffect::Logout => "logout",
            ClientEffect::LoginRequest(_) => "login",
            ClientEffect::LobbyEnterGame(_) => "lobby",
            ClientEffect::CreateConnect => "create",
            _ => "other",
        })
        .collect();
    assert_eq!(
        kinds,
        ["url", "cheat", "quit", "switch", "cancel", "logout", "login", "lobby", "create"]
    );
    let (after_title, now): (Vec<_>, Vec<_>) = effects
        .into_iter()
        .partition(ClientEffect::after_title_screen);
    assert_eq!((now.len(), after_title.len()), (8, 1));
    assert!(engine.login.world_switch.is_none() && engine.builtins.requests.is_empty());
}

/// Phase 4.4: an interface packet batch always yields its appliers in the
/// old order, empty or not (the hint-arrow rebase runs once per batch).
#[test]
fn packet_effects_keep_the_old_apply_order() {
    let kinds = |effects: Vec<ClientEffect>| -> Vec<&'static str> {
        effects
            .iter()
            .map(|e| match e {
                ClientEffect::PointLights(_) => "lights",
                ClientEffect::EnvironmentOverrides(_) => "environment",
                ClientEffect::HintArrows(_) => "hints",
                ClientEffect::BrowserUrls { .. } => "urls",
                ClientEffect::ConsoleMessages(_) => "console",
                _ => "other",
            })
            .collect()
    };
    assert_eq!(
        kinds(packet_effects(None, true)),
        ["lights", "environment", "hints", "urls", "console"]
    );
    assert_eq!(
        kinds(packet_effects(None, false)),
        ["lights", "environment", "hints", "urls"]
    );
}
