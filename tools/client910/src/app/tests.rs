#[test]
fn socket_receives_later_packets_after_startup_reactor_is_dropped() -> anyhow::Result<()> {
    use std::io::{ErrorKind, Read, Write};
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let stream = runtime.block_on(tokio::net::TcpStream::connect(listener.local_addr()?))?;
    let (mut peer, _) = listener.accept()?;
    let mut stream =
        super::detach_world_socket(rs910_client::wire_stream::WireStream::plain(stream))?;
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

// --- Live client state over the real cache ---

/// Logout resets the transmit counters: a relog starts every transmit counter at zero while the client variable
/// values themselves survive.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn logout_resets_transmit_nums_but_keeps_client_variables() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.interfaces.js5");
    let mut game = rs910_ui::client_game::ClientGame::login(
        &pack,
        1,
        rs910_game::protocol910::live::Feed::default(),
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
        .insert(1234, rs910_ui::ui_vars::Value::Int(77));
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
        Some(&rs910_ui::ui_vars::Value::Int(77))
    );
    let mut cycles = rs910_ui::ui_loop::Cycles {
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
        rs910_ui::client_game::ClientGame::login(
            &pack,
            local,
            rs910_game::protocol910::live::Feed::default(),
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
        .insert(1234, rs910_ui::ui_vars::Value::Int(77));
    let mut next = login(2)?;
    assert!(adopt_client_variables(Some(previous), &mut next, false)?);
    assert_eq!(next.runtime.varp_transmit_num, 41);
    assert_eq!(next.runtime.varp_transmitted[40], 3814);
    assert_eq!(
        next.ui_variables.client.values.get(&1234),
        Some(&rs910_ui::ui_vars::Value::Int(77))
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
    let mut ui = rs910_ui::ui_runtime::Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    rs910_ui::client_game::with_game(&mut game, |v| {
        ui.show_top_level(v, interface::LOGIN_SCREEN.id())
    })?;
    assert_eq!(ui.state.life.top, interface::LOGIN_SCREEN.id());
    // LoginLayout.ts (lobby): the top 906, then 907 in 906:107.
    let lobby_sub = component::lobby_window::PLAYER_INFO_SLOT.packed();
    rs910_ui::client_game::with_game(&mut game, |v| {
        ui.packet(
            v,
            &rs910_client::session::UiEvent::OpenTop {
                interface_id: interface::LOBBY_WINDOW.id() as u32,
                keys: [0; 4],
            },
        )?;
        ui.packet(
            v,
            &rs910_client::session::UiEvent::OpenSub {
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
    rs910_ui::client_game::with_game(&mut game, |v| {
        ui.show_top_level(v, interface::LOGIN_SCREEN.id())
    })?;
    assert_eq!(ui.state.life.top, interface::LOGIN_SCREEN.id());
    assert!(ui.state.life.subs.get(lobby_sub).is_none());
    assert!(!ui
        .store
        .interfaces
        .contains_key(&interface::LOBBY_WINDOW.id()));
    // Lobby reply -> setState(13) -> showLobby(true) with lobby_interface -1.
    rs910_ui::client_game::with_game(&mut game, |v| ui.show_top_level(v, -1))?;
    assert_eq!(ui.state.life.top, -1);
    assert!(ui.store.interfaces.is_empty());
    assert_eq!(ui.diagnostics.failures, 0);
    Ok(())
}
