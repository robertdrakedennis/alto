//! Engine command cases and the command partition they cover.

use super::super::ActiveEntity;

use super::{c, cam2_mode, rec, Case, Obs, Set, World};

/// Commands the engine itself owns (`ui_runtime::commands`: marketing,
/// notifications, login/lobby session, world list, stockmarket, social
/// presence, local-player coordinates). Recorded rows run from the harness's
/// default state (fresh login, `state` 0 unless set); hand rows install the
/// owner state the recording harness has no directive for.
pub(super) fn engine_partition() -> Vec<Case> {
    let fine = || crate::ui_cam2::encode_coord_fine(2, [45_678, -321, 54_321]);
    let trackable = |w: &mut World| {
        w.engine.camera.cam2.scene.local_player = Some(crate::ui_cam2::Trackable {
            kind: crate::ui_cam2::TRACKABLE_PLAYER,
            index: 2,
            level: 2,
            coord: [45_678, -321, 54_321],
            yaw: 0,
        })
    };
    // State 0x0d is a sell offer (bit 8) in
    // status 5 (finished), at stockmarketSlots[2][3].
    let market = |w: &mut World| {
        w.engine.stockmarket_slots[2][3] = super::super::StockmarketSlot {
            state: 0x0d,
            object: 101,
            price: 202,
            count: 303,
            completed_count: 404,
            completed_gold: 505,
        }
    };
    let mut v = vec![
        // Empty handlers; sendevent only pops its argument.
        rec("marketing_init"),
        rec("notifications_init"),
        rec("notifications_opensettings"),
        rec("marketing_sendevent").s(&["event"]),
        rec("map_quickchat"),
        c("map_quickchat")
            .with(|w| w.engine.account.logged_in_quickchat = true)
            .wi(&[1]),
        // get_npc_stat on the active NPC: getStat, getStatMax.
        c("get_npc_stat")
            .with(|w| {
                w.engine.scene.active_entity = Some(ActiveEntity::Npc {
                    index: 7,
                    type_id: 42,
                    name: "Goblin".into(),
                    chat: None,
                    stats: [10, 30, 0, 0, 0, 0],
                    stat_max: [20, 99, 0, 0, 0, 0],
                    vislevel: 2,
                    active: true,
                    overlay_height: 100,
                    target: -1,
                    position: [0.0; 3],
                    screen_bounds: None,
                })
            })
            .i(&[1])
            .wi(&[30, 99]),
        // nc_param: pops (npc, param); an int param absent from
        // the NPC type answers defaultint.
        c("nc_param")
            .with(|w| {
                w.engine.configs.params.insert(
                    42,
                    native910::config::ParamConfig {
                        kind: Some(0),
                        default_int: Some(7),
                        ..Default::default()
                    },
                );
            })
            .i(&[123, 42])
            .wi(&[7]),
        // lobby_enterlobby → enterLobby: nothing
        // outside the login-ready state, a retained request inside it.
        rec("lobby_enterlobby")
            .i(&[1])
            .s(&["Alice", "secret", "trusted"])
            .check(|w| {
                (w.engine.login.request.is_none() && !w.engine.login.in_progress)
                    .then_some(())
                    .ok_or(format!("{:?}", w.engine.login.request))
            }),
        c("lobby_enterlobby")
            .with(|w| w.engine.login.ready = true)
            .i(&[1])
            .s(&["Alice", "secret", "trusted"])
            .check(|w| {
                let want = super::super::LoginRequest {
                    username: "Alice".into(),
                    password: "secret".into(),
                    new_auth_preference: "trusted".into(),
                    auth_dont_trust: true,
                    lobby: true,
                    sso: None,
                };
                (w.engine.login.request.as_ref() == Some(&want) && w.engine.login.in_progress)
                    .then_some(())
                    .ok_or(format!("{:?}", w.engine.login.request))
            }),
        rec("login_inprogress"),
        c("login_inprogress")
            .with(|w| w.engine.login.in_progress = true)
            .wi(&[1]),
        // Login state defaults.
        rec("login_reply"),
        rec("lobby_entergamereply"),
        rec("lobby_enterlobbyreply"),
        rec("login_hoptime"),
        rec("login_ban_duration"),
        rec("login_disallowresult"),
        rec("login_disallowtrigger"),
        // userflowflags/automatedtestflags push the high word first.
        rec("userflowflags"),
        c("userflowflags")
            .with(|w| w.engine.login.user_flow = [12, 34])
            .wi(&[12, 34]),
        rec("automatedtestflags"),
        rec("login_cancel"),
        c("login_cancel")
            .with(|w| w.engine.login.in_progress = true)
            .check(|w| {
                (w.engine.login.cancel_requested && !w.engine.login.in_progress)
                    .then_some(())
                    .ok_or("login not cancelled".into())
            }),
        // login_resetreply: resetLoginState unless in progress.
        rec("login_resetreply"),
        c("login_resetreply")
            .with(|w| {
                let l = &mut w.engine.login;
                (
                    l.reply,
                    l.lobby_reply,
                    l.hoptime,
                    l.ban_duration,
                    l.queue_position,
                ) = (3, 3, 500, 7, 9);
            })
            .check(|w| {
                let l = &w.engine.login;
                let got = (
                    l.reply,
                    l.lobby_reply,
                    l.hoptime,
                    l.ban_duration,
                    l.queue_position,
                );
                (got == (-2, -2, 0, 0, -1))
                    .then_some(())
                    .ok_or(format!("{got:?}"))
            }),
        c("login_resetreply")
            .with(|w| {
                w.engine.login.in_progress = true;
                w.engine.login.reply = 3;
            })
            .check(|w| {
                (w.engine.login.reply == 3)
                    .then_some(())
                    .ok_or("reply reset while in progress".into())
            }),
        // worldlist_pingworlds pops only in the lobby (state 13).
        rec("worldlist_pingworlds").i(&[1]),
        c("worldlist_pingworlds")
            .with(|w| w.engine.login.lobby_login = true)
            .i(&[1])
            .check(|w| {
                w.engine
                    .world_list
                    .resolve_hosts_enabled
                    .then_some(())
                    .ok_or("resolveHostsEnabled not set".into())
            }),
        // worldlist_autoworld → restoreWorld.
        c("worldlist_autoworld")
            .with(|w| {
                w.engine.login.target_world = Some(super::super::WorldSwitchRequest {
                    world_id: 42,
                    host: "world42".into(),
                });
                w.engine.login.world = 1;
            })
            .check(|w| {
                (w.engine.login.world == 42)
                    .then_some(())
                    .ok_or(format!("world {}", w.engine.login.world))
            }),
        rec("lobby_enterlobby_sso").i(&[1]).s(&["token"]),
        rec("sso_available"),
        rec("sso_displayname"),
        // lobby_entergame → enterGame from the lobby.
        c("lobby_entergame")
            .with(|w| w.engine.login.lobby_login = true)
            .i(&[1])
            .s(&["trusted"])
            .check(|w| {
                let want = super::super::LobbyEnterGameRequest {
                    new_auth_preference: "trusted".into(),
                    auth_dont_trust: true,
                };
                (w.engine.login.lobby_enter_game.as_ref() == Some(&want)
                    && w.engine.login.in_progress)
                    .then_some(())
                    .ok_or(format!("{:?}", w.engine.login.lobby_enter_game))
            }),
        // lobby_leavelobby → logout(false).
        c("lobby_leavelobby").check(|w| {
            w.engine
                .login
                .logout_requested
                .then_some(())
                .ok_or("logout not requested".into())
        }),
        // worldlist_fetch: 1 outside state 13/18 or while a
        // login is in progress; otherwise WORLDLIST_FETCH p4(token) and 0.
        rec("worldlist_fetch").obs(Obs::Out),
        rec("worldlist_fetch").set(Set::State(18)).obs(Obs::Out),
        c("worldlist_fetch")
            .with(|w| {
                w.engine.login.lobby_login = true;
                w.engine.login.in_progress = true;
            })
            .wi(&[1])
            .wo(Obs::Out, ""),
        // login_last_transfer_reply pushes then resets.
        rec("login_last_transfer_reply"),
        c("login_last_transfer_reply")
            .with(|w| {
                let l = &mut w.engine.login;
                (
                    l.last_transfer_reply,
                    l.last_transfer_disallow_result,
                    l.last_transfer_disallow_trigger,
                ) = (23, 4, 8);
            })
            .wi(&[23, 4, 8])
            .check(|w| {
                let l = &w.engine.login;
                let got = (
                    l.last_transfer_reply,
                    l.last_transfer_disallow_result,
                    l.last_transfer_disallow_trigger,
                );
                (got == (-2, -1, -1))
                    .then_some(())
                    .ok_or(format!("{got:?}"))
            }),
        // lobby* profile defaults.
        rec("userdetail_lobby_recoveryday"),
        rec("userdetail_lobby_playage"),
        // Social presence.
        rec("player_group_find"),
        c("player_group_find")
            .with(|w| w.engine.social.player_group_present = true)
            .wi(&[1]),
        rec("activeclanchannel_find_affined"),
        rec("activeclanchannel_find_affined")
            .set(Set::ClanUser("Bob", 1))
            .check(|w| {
                let active = w.engine.social.active_channel.as_ref();
                active
                    .is_some_and(|c| c.users.first().is_some_and(|u| u.name == "Bob"))
                    .then_some(())
                    .ok_or(format!("{active:?}"))
            }),
        rec("chat_playername"),
        rec("chat_playername_unfiltered"),
        c("chat_playername")
            .with(|w| w.engine.social.local_player_name = "Alice".into())
            .ws(&["Alice"]),
        c("chat_playername_unfiltered")
            .with(|w| w.engine.social.local_player_name = "Alice".into())
            .ws(&["Alice"]),
        // The local player's grid / fine coordinates.
        c("getgridcoordrelativetocamera")
            .with(trackable)
            .i(&[1234])
            .wi(&[(2 << 28) | (89 << 14) | 106]),
        c("coord_fine").with(trackable).ws(&[&fine()]),
        // cam2_setpositionpoint_point takes that FineCoord.
        c("cam2_setpositionpoint_point")
            .with(|w| cam2_mode(w, "cam2_setpositionmode", crate::ui_cam2::MODE_POINT))
            .s(&[&fine()])
            .check(|w| match &w.engine.camera.cam2.position {
                Some(crate::ui_cam2::Position::Point(p))
                    if p.level == 2
                        && (p.current.x, p.current.y, p.current.z)
                            == (45_678.0, -321.0, 54_321.0) =>
                {
                    Ok(())
                }
                other => Err(format!("{other:?}")),
            }),
    ];
    // stockmarket_* pop (slot, market) and read slots[market][slot].
    for (cmd, want) in [
        ("stockmarket_getoffertype", 1),
        ("stockmarket_getofferitem", 101),
        ("stockmarket_getofferprice", 202),
        ("stockmarket_getoffercount", 303),
        ("stockmarket_getoffercompletedcount", 404),
        ("stockmarket_getoffercompletedgold", 505),
        ("stockmarket_isofferempty", 0),
        ("stockmarket_isofferstable", 0),
        ("stockmarket_isofferfinished", 1),
        ("stockmarket_isofferadding", 0),
    ] {
        v.push(c(cmd).with(market).i(&[3, 2]).wi(&[want]));
    }
    v
}

/// Engine-owned commands that must keep a behaviour row in
/// [`engine_partition`].
pub(super) const ENGINE_PARTITION: &[&str] = &[
    "marketing_init",
    "notifications_init",
    "notifications_opensettings",
    "marketing_sendevent",
    "map_quickchat",
    "get_npc_stat",
    "nc_param",
    "lobby_enterlobby",
    "login_inprogress",
    "login_reply",
    "lobby_entergamereply",
    "lobby_enterlobbyreply",
    "login_hoptime",
    "login_ban_duration",
    "login_disallowresult",
    "login_disallowtrigger",
    "userflowflags",
    "automatedtestflags",
    "login_cancel",
    "login_resetreply",
    "worldlist_pingworlds",
    "worldlist_autoworld",
    "lobby_enterlobby_sso",
    "sso_available",
    "sso_displayname",
    "lobby_entergame",
    "lobby_leavelobby",
    "worldlist_fetch",
    "login_last_transfer_reply",
    "stockmarket_getoffertype",
    "stockmarket_getofferitem",
    "stockmarket_getofferprice",
    "stockmarket_getoffercount",
    "stockmarket_getoffercompletedcount",
    "stockmarket_getoffercompletedgold",
    "stockmarket_isofferempty",
    "stockmarket_isofferstable",
    "stockmarket_isofferfinished",
    "stockmarket_isofferadding",
    "player_group_find",
    "activeclanchannel_find_affined",
    "chat_playername",
    "chat_playername_unfiltered",
    "getgridcoordrelativetocamera",
    "coord_fine",
];
