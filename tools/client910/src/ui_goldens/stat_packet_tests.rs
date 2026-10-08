use crate::client_game::with_game;
use crate::ui_runtime::*;
use crate::ui_vars::Variables;
use crate::{cache::Pack, protocol910::live::Feed, session::parse_ui_event};
use native910::vm::Value;

/// A game-message packet feeds the chat history:
/// one packet is exactly one history line. (The login "Welcome to
/// RuneScape." line is cache script 1299's `mes`, not this packet.)
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn one_game_message_is_one_chat_line() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    // gSmart1or2 type 0, g4s flags 0, no names, "It's a Man."
    let mut payload = vec![0, 0, 0, 0, 0, 0];
    payload.extend_from_slice(b"It's a Man.\0");
    let event =
        parse_ui_event(crate::proto::server::MESSAGE_GAME, &payload)?.expect("game-message event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(ui.engine.messages.history.length(0), 1);
    assert_eq!(ui.engine.messages.history.last_uid(), 0);
    let line = ui
        .engine
        .messages
        .history
        .get_by_uid(0)
        .expect("history line");
    assert_eq!((line.chat_type, line.message.as_str()), (0, "It's a Man."));
    assert_eq!(ui.engine.messages.history.previous_uid(0), -1);
    // The packet boundary turns the change into the chat redraw stamp.
    assert_eq!(ui.state.life.cycles.chat, ui.state.life.cycles.redraw);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn private_message_reaches_chat_and_deduplicates() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    // MESSAGE_PRIVATE: no quick-chat flag, sender, 40-bit message id,
    // PLAYER_MOD crown, then a valid zero-length word-pack body.
    let payload = [0, b'B', b'o', b'b', 0, 0, 1, 0, 0, 7, 1, 0];
    let event = parse_ui_event(crate::proto::server::MESSAGE_PRIVATE, &payload)?
        .expect("private-message event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    let line = ui
        .engine
        .messages
        .history
        .get_by_type_and_line(7, 0)
        .expect("private chat line");
    assert_eq!(line.name, "<img=0>Bob");
    assert_eq!(line.name_simple, "Bob");
    assert_eq!(line.message, "");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(ui.engine.messages.history.length(7), 1);
    let packed = (10 << 14) | 20;
    let roof_wire = [
        (packed >> 8) as u8,
        packed as u8,
        (packed >> 24) as u8,
        (packed >> 16) as u8,
    ];
    let roof_event = parse_ui_event(crate::proto::server::CAM_REMOVEROOF, &roof_wire)?
        .expect("server roof event");
    with_game(&mut game, |vars| ui.packet(vars, &roof_event))?;
    assert_eq!(
        ui.engine.scene.server_roof,
        [10 * 512 + 256, 20 * 512 + 256]
    );
    let reset = parse_ui_event(crate::proto::server::CAM_SMOOTHRESET, &[])?
        .expect("camera smooth reset event");
    with_game(&mut game, |vars| ui.packet(vars, &reset))?;
    assert_eq!(ui.engine.scene.server_roof, [-1, -1]);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn public_message_feeds_overhead_chat_and_the_gates() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let mut player = crate::entities910::Player::default();
    player.appearance.name = Some("Alice".into());
    player.appearance.model = Some(crate::entities910::appearance::Model {
        bas: -1,
        kits: Vec::new(),
        custom: Vec::new(),
        colours: [0; 10],
        textures: [0; 10],
        female: false,
        npc: -1,
        hash: 0,
    });
    game.runtime.feed.state.players.players[5] = Some(player);
    // MESSAGE_PUBLIC (8/-1): g2 index 5, g2 flags colour 9 (wave-glow)
    // << 8 | effect 2 (wave2), crown PLAYER_MOD (1), empty word-pack body.
    let payload = [0, 5, 9, 2, 1, 0];
    let event = parse_ui_event(crate::proto::server::MESSAGE_PUBLIC, &payload)?
        .expect("public-message event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(ui.engine.effects.overhead_chat.len(), 1);
    crate::app::apply_overhead_chat(&mut game, &mut ui.engine.effects.overhead_chat);
    assert!(ui.engine.effects.overhead_chat.is_empty());
    let chat = game.runtime.feed.state.players.players[5]
        .as_ref()
        .and_then(|p| p.chat.clone())
        .expect("the public message installed a chat line");
    assert_eq!(chat.text.as_deref(), Some(""));
    assert_eq!((chat.colour, chat.effect), (9, 2));
    let limits = game
        .inputs
        .appearance
        .defaults
        .graphics
        .entity_limits(game.inputs.logic_rate);
    assert_eq!(chat.total, limits.player_chat_ticks);
    let line = ui
        .engine
        .messages
        .history
        .get_by_type_and_line(1, 0)
        .expect("public chat line");
    assert_eq!(line.flags, 0, "public chat lines are added with flags 0");
    assert_eq!(line.name, "<img=0>Alice");
    assert_eq!(line.crown, Some(1), "the crown id, not the image");
    // An ignored sender is dropped before either consumer.
    ui.engine
        .social
        .ignores
        .push(crate::ui_social::IgnoreEntry {
            name_unfiltered: "alice".into(),
            ..Default::default()
        });
    game.runtime.feed.state.players.players[5]
        .as_mut()
        .unwrap()
        .chat = None;
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert!(ui.engine.effects.overhead_chat.is_empty());
    assert_eq!(ui.engine.messages.history.length(1), 1);
    // STAFF_MOD (2) is not ignorable.
    let staff = [0, 5, 0, 0, 2, 0];
    let event = parse_ui_event(crate::proto::server::MESSAGE_PUBLIC, &staff)?
        .expect("public-message event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    crate::app::apply_overhead_chat(&mut game, &mut ui.engine.effects.overhead_chat);
    assert!(game.runtime.feed.state.players.players[5]
        .as_ref()
        .unwrap()
        .chat
        .is_some());
    // A private message from the ignored name is dropped without recording
    // its id; the shared ring still deduplicates other families.
    let private = [0, b'A', b'l', b'i', b'c', b'e', 0, 0, 0, 0, 0, 9, 0, 0];
    let event = parse_ui_event(crate::proto::server::MESSAGE_PRIVATE, &private)?
        .expect("private-message event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(ui.engine.messages.history.length(3), 0);
    assert!(!ui.engine.messages.message_seen(9));
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn hint_trail_packet_reaches_retained_scene_owner() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let payload = [3, 0, 42, 66, 0x0c, 0x80, 0x0c, 0x81, 1, 255, 0, 2];
    let event =
        parse_ui_event(crate::proto::server::HINT_TRAIL, &payload)?.expect("hint-trail event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(
        ui.engine.scene.hint_trails[3],
        Some(HintTrail {
            model: 42,
            points: vec![[3201, 3200], [3201, 3202]],
        })
    );
    let clear = [3, 0x7f, 0xff];
    let event =
        parse_ui_event(crate::proto::server::HINT_TRAIL, &clear)?.expect("hint-trail clear event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(ui.engine.scene.hint_trails[3], None);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn varclan_packet_reaches_sparse_domain_and_cs2_reads() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let (&id, _) = game
        .inputs
        .bits
        .definitions
        .get(&6)
        .into_iter()
        .flat_map(|defs| defs.iter())
        .find(|(_, def)| {
            def.data_type
                .and_then(crate::protocol910::script_types::script_type)
                .is_some_and(|(base, _)| base == 0)
        })
        .ok_or_else(|| anyhow::anyhow!("pack has no integer clan variable definition"))?;
    let value = 0x1234_5678_i32;
    let mut payload = (id as u16).to_be_bytes().to_vec();
    payload.extend_from_slice(&value.to_be_bytes());
    let event = parse_ui_event(crate::proto::server::VARCLAN, &payload)?.expect("VARCLAN event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(game.ui_variables.varclan_transmit.count, 1);
    assert!(ui.engine.social.clan_vars.is_some());
    let observed = with_game(&mut game, |vars| {
        vars.get(native910::vars::VarScope::Clan, id as u16, false)
    })?;
    assert_eq!(observed, native910::vm::Value::Int(value));
    Ok(())
}

/// After `activeclansettings_find_affined`, domain-7 reads resolve the
/// active settings map keyed `modegame << 16 | id`; absent settings read
/// the var default and every setter throws.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn clan_settings_var_domain_reads_active_settings() -> anyhow::Result<()> {
    use native910::vars::VarScope;
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let int_vars: Vec<i32> = game
        .inputs
        .bits
        .definitions
        .get(&7)
        .map(|defs| {
            defs.values()
                .filter(|d| {
                    d.data_type
                        .and_then(crate::protocol910::script_types::script_type)
                        .is_some_and(|t| t.0 == 0)
                })
                .map(|d| d.id)
                .collect()
        })
        .unwrap_or_default();
    assert!(int_vars.len() >= 2, "cache has int clan-setting vars");
    let (set_id, unset_id) = (int_vars[0], int_vars[1]);
    // Before any CLANSETTINGS packet no domain is installed.
    assert!(with_game(&mut game, |vars| vars.get(
        VarScope::ClanSetting,
        set_id as u16,
        false
    ))
    .is_err());
    let key = crate::applet_params::get().mode_game_id() << 16 | set_id;
    let mut full = vec![1, 3, 0];
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend_from_slice(&0_u16.to_be_bytes());
    full.extend([0]);
    full.extend_from_slice(b"Test\0");
    full.extend([0, 0, 0, 0, 0]);
    full.extend_from_slice(&1_u16.to_be_bytes());
    full.extend_from_slice(&(key as u32 & 0x3fff_ffff).to_be_bytes());
    full.extend_from_slice(&4321_i32.to_be_bytes());
    let event = parse_ui_event(crate::proto::server::CLANSETTINGS_FULL, &full)?
        .expect("CLANSETTINGS_FULL event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    // Linked but no activeClanSettings yet: still uninstalled.
    assert!(with_game(&mut game, |vars| vars.get(
        VarScope::ClanSetting,
        set_id as u16,
        false
    ))
    .is_err());
    let (mut ints, mut objs) = (Vec::new(), Vec::new());
    ui.engine
        .social
        .dispatch("activeclansettings_find_affined", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    let value = with_game(&mut game, |vars| {
        vars.get(VarScope::ClanSetting, set_id as u16, false)
    })?;
    assert_eq!(value, native910::vm::Value::Int(4321));
    let default = game.inputs.bits.binding(7, unset_id).unwrap().unwrap();
    let expected = match default.default_value().unwrap() {
        crate::protocol910::variables::Value::Int(v) => v,
        other => panic!("int default {other:?}"),
    };
    let value = with_game(&mut game, |vars| {
        vars.get(VarScope::ClanSetting, unset_id as u16, false)
    })?;
    assert_eq!(value, native910::vm::Value::Int(expected));
    assert!(with_game(&mut game, |vars| vars.set(
        VarScope::ClanSetting,
        set_id as u16,
        false,
        native910::vm::Value::Int(1)
    ))
    .is_err());
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn clan_settings_full_and_delta_reach_active_queries() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let mut full = vec![1, 6, 2, 0, 0, 0, 1, 0, 0, 0, 0, 0, 2, 1];
    full.extend_from_slice(b"Test\0");
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend([1, 2, 3, 4, 5]);
    full.extend_from_slice(b"Alice\0");
    full.extend([5]);
    full.extend_from_slice(&0x1234_i32.to_be_bytes());
    full.extend_from_slice(&42_u16.to_be_bytes());
    full.extend([0]);
    full.extend_from_slice(b"Bob\0");
    full.extend([4]);
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend_from_slice(&43_u16.to_be_bytes());
    full.extend([1]);
    full.extend_from_slice(b"Bad\0");
    full.extend_from_slice(&1_u16.to_be_bytes());
    full.extend_from_slice(&7_u32.to_be_bytes());
    full.extend_from_slice(&99_i32.to_be_bytes());
    let event = parse_ui_event(crate::proto::server::CLANSETTINGS_FULL, &full)?
        .expect("CLANSETTINGS_FULL event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    let mut ints = Vec::new();
    let mut objs = Vec::new();
    ui.engine
        .social
        .dispatch("activeclansettings_find_affined", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ui.engine
        .social
        .dispatch("activeclansettings_getclanname", &mut ints, &mut objs)?;
    assert_eq!(objs.pop().as_deref(), Some("Test"));
    ui.engine
        .social
        .dispatch("activeclansettings_getaffinedcount", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(2));
    ints.push(0);
    ui.engine
        .social
        .dispatch("activeclansettings_getaffinedrank", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(126));
    ints.push(1);
    ui.engine
        .social
        .dispatch("activeclansettings_getaffinedrank", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(4));

    let mut delta = vec![1];
    delta.extend_from_slice(&0_i64.to_be_bytes());
    delta.extend_from_slice(&1_i32.to_be_bytes());
    delta.extend([2, 0, 1, 7, 4, 0, 1, 2, 3, 4, 0]);
    let event = parse_ui_event(crate::proto::server::CLANSETTINGS_DELTA, &delta)?
        .expect("CLANSETTINGS_DELTA event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    ints.push(1);
    ui.engine
        .social
        .dispatch("activeclansettings_getaffinedrank", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(7));
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn player_group_full_and_delta_reach_member_and_var_queries() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let group_var = game
        .inputs
        .bits
        .definitions
        .get(&9)
        .into_iter()
        .flat_map(|defs| defs.iter())
        .find(|(_, def)| {
            def.data_type
                .and_then(crate::protocol910::script_types::script_type)
                .is_some_and(|(base, _)| base == 0)
        })
        .map(|(id, _)| *id)
        .ok_or_else(|| anyhow::anyhow!("pack has no integer player-group variable definition"))?;
    let member_var = game
        .inputs
        .bits
        .definitions
        .get(&0)
        .into_iter()
        .flat_map(|defs| defs.iter())
        .find(|(_, def)| {
            def.data_type
                .and_then(crate::protocol910::script_types::script_type)
                .is_some_and(|(base, _)| base == 0)
        })
        .map(|(id, _)| *id)
        .ok_or_else(|| anyhow::anyhow!("pack has no integer player variable definition"))?;
    let mut full = vec![1, 5];
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend_from_slice(&0_i64.to_be_bytes());
    full.extend_from_slice(b"Group\0");
    full.extend_from_slice(&10_i16.to_be_bytes());
    full.extend_from_slice(&0_i32.to_be_bytes());
    full.extend_from_slice(&0_i64.to_be_bytes());
    full.extend_from_slice(&1_u16.to_be_bytes());
    full.extend_from_slice(b"Alice\0");
    full.extend([3, 0, 0, 0, 0, 42, 5, 3, 2]);
    full.extend_from_slice(&1_u16.to_be_bytes());
    full.extend_from_slice(b"Bad\0");
    full.extend_from_slice(&0_u16.to_be_bytes());
    let event = parse_ui_event(crate::proto::server::PLAYER_GROUP_FULL, &full)?
        .expect("PLAYER_GROUP_FULL event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    let mut ints = Vec::new();
    let mut objs = Vec::new();
    ui.engine
        .social
        .dispatch("player_group_find", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ui.engine
        .social
        .dispatch("player_group_member_count", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_get_displayname", &mut ints, &mut objs)?;
    assert_eq!(objs.pop().as_deref(), Some("Alice"));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_is_owner", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));

    let mut delta = vec![0_u8; 8];
    delta.extend_from_slice(&0_i32.to_be_bytes());
    delta.extend([5, 0, 0, 7, 12, (group_var >> 8) as u8, group_var as u8]);
    delta.extend_from_slice(&123_i32.to_be_bytes());
    delta.push(0);
    let event = parse_ui_event(crate::proto::server::PLAYER_GROUP_DELTA, &delta)?
        .expect("PLAYER_GROUP_DELTA event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_get_rank", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(7));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_get_status", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(3));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_get_team", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(2));
    ints.push(0);
    ui.engine.social.dispatch(
        "player_group_member_get_last_seen_node_id",
        &mut ints,
        &mut objs,
    )?;
    assert_eq!(ints.pop(), Some(42));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_is_online", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_member_is_member", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ints.extend([0, 0, 0]);
    ui.engine
        .social
        .dispatch("player_group_member_get_join_xp", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(0));
    ui.engine
        .social
        .dispatch("player_group_banned_count", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(1));
    ints.push(0);
    ui.engine
        .social
        .dispatch("player_group_banned_get_displayname", &mut ints, &mut objs)?;
    assert_eq!(objs.pop().as_deref(), Some("Bad"));
    ui.engine
        .social
        .dispatch("player_group_get_displayname", &mut ints, &mut objs)?;
    assert_eq!(objs.pop().as_deref(), Some("Group"));
    ui.engine
        .social
        .dispatch("player_group_get_max_size", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(10));
    ui.engine
        .social
        .dispatch("player_group_get_overall_status", &mut ints, &mut objs)?;
    assert_eq!(ints.pop(), Some(3));
    let value = with_game(&mut game, |vars| {
        vars.get(native910::vars::VarScope::Group, group_var as u16, false)
    })?;
    assert_eq!(value, native910::vm::Value::Int(123));
    let mut member_vars = vec![0, 0, 1];
    member_vars.extend_from_slice(&(member_var as u16).to_be_bytes());
    member_vars.extend_from_slice(&321_i32.to_be_bytes());
    let event = parse_ui_event(crate::proto::server::PLAYER_GROUP_VARPS, &member_vars)?
        .expect("PLAYER_GROUP_VARPS event");
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    // The packet writes the separate clearable container that the
    // same-world query reads.
    assert_eq!(
        ui.engine
            .social
            .player_group
            .as_ref()
            .and_then(|group| group.members[0].variables.as_ref())
            .and_then(|variables| variables.get(&member_var)),
        Some(&crate::ui_vars::Value::Int(321))
    );
    ints.extend([0, 1, member_var]);
    assert_eq!(
        ui.engine.social.dispatch(
            "player_group_member_get_same_world_var",
            &mut ints,
            &mut objs
        )?,
        Some(Value::Int(321))
    );
    Ok(())
}

/// Actual server login/menu state through ground picking, retained menu
/// construction and outgoing FACE_SQUARE, including scaled viewports.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn navigation_packets_reach_face_here_menu_output() -> anyhow::Result<()> {
    let rows = crate::test_support::replay_json("navigation-menu", "frames.json");
    let path = crate::test_support::proof_dir("navigation-menu");
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([1280, 720])?;
    ui.engine.account.logged_in_members = true;
    ui.target.quiet = true;
    ui.diagnostics.capture = true;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, true)?;
    let mut millis = 10_000i64;
    fn clock<T>(
        game: &mut crate::client_game::ClientGame,
        millis: i64,
        f: impl FnOnce(&mut Variables<'_>) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let mut now = || millis;
        f(&mut Variables {
            cycle: game.game.cycle,
            definitions: &game.game.inputs.bits,
            state: &mut game.ui_variables,
            player: game.game.runtime.feed.state.varps.as_mut(),
            active_player: None,
            active_npc: None,
            now: &mut now,
            probe: None,
            varp_transmit: crate::ui_loop::Counter {
                num: game.game.runtime.varp_transmit_num,
                ids: game.game.runtime.varp_transmitted,
            },
            scene: crate::ui_cam2::SceneInput::new(
                &game.game.runtime.map,
                &game.game.runtime.feed.state.players,
                game.game.runtime.terrain.as_ref(),
                game.game.runtime.terrain_generation,
            ),
        })
    }
    for row in rows.as_array().unwrap() {
        for wire in row["frames"].as_array().unwrap() {
            let bytes: Vec<u8> = wire
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect();
            let (frame, n) = crate::net::decode_frame(&bytes)?.unwrap();
            assert_eq!(n, bytes.len());
            if game.runtime.feed.enqueue(frame.opcode, &frame.payload) {
                game.apply_next(millis)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                if game.runtime.map_request.is_some() {
                    let map = game.runtime.prepare_map(&pack)?;
                    game.runtime
                        .install_map(map)
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                }
            } else if let Some(event) = parse_ui_event(frame.opcode, &frame.payload)? {
                let verify = ui.state.life.verify;
                clock(&mut game, millis, |v| ui.packet(v, &event))?;
                if matches!(event, crate::session::UiEvent::ShowFaceHere { .. }) {
                    assert_eq!(ui.state.life.verify, verify.wrapping_add(1));
                    assert!(ui.state.life.verify_changed);
                }
            } else {
                anyhow::bail!("unhandled navigation opcode {}", frame.opcode);
            }
        }
        for _ in 0..50 {
            millis += 20;
            game.cycle += 1;
            game.poll_vars(|| millis)
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            game.update_actors().map_err(|e| anyhow::anyhow!("{e:?}"))?;
            clock(&mut game, millis, |v| ui.tick(v))?;
        }
        match row["label"].as_str().unwrap() {
            "login" | "face-enabled" => assert!(ui.engine.menu.show_face_here),
            "custom" => {
                assert_eq!(ui.engine.menu.walk_here_text, "Go here");
                assert_eq!(ui.engine.menu.default_walk_action, 123);
                let op = ui.engine.menu.player_ops[2].as_ref().unwrap();
                assert_eq!(op.name.as_deref(), Some("Follow"));
                assert_eq!(op.cursor, 65534);
                assert!(op.deprioritised);
            }
            "reset" => {
                assert_eq!(ui.engine.menu.walk_here_text, "Walk here");
                assert_eq!(ui.engine.menu.default_walk_action, -1);
                assert!(ui.engine.menu.player_ops[2]
                    .as_ref()
                    .unwrap()
                    .name
                    .is_none());
                assert!(!ui.engine.menu.show_face_here);
            }
            _ => unreachable!(),
        }
    }
    let mut proof = vec![];
    for canvas in [[1280, 720], [1600, 900]] {
        ui.resize(canvas)?;
        clock(&mut game, millis, |v| ui.tick(v))?;
        let _ = ui.paint(game.cycle, true, [0.05; 3])?;
        let _ = ui.paint(game.cycle, true, [0.05; 3])?;
        let (viewport, _) = ui.state.viewport.expect("actual scene viewport");
        let base = ui.engine.camera.cam2.scene.base;
        let tile = [3225 - (base[0] >> 9), 3222 - (base[1] >> 9)];
        let h = ui.engine.camera.cam2.scene.heightmap.as_ref().unwrap();
        let fine = [tile[0] * 512 + 256, tile[1] * 512 + 256];
        let y = crate::ui_scene_options::fine_height(h, 0, fine[0], fine[1]);
        let frame = ui.engine.camera.cam2.frame().expect("server-driven camera");
        let mut rel = frame.clone();
        rel.rebase([base[0], 0, base[1]]);
        let view = rel.view_matrix([0, 0, 0]).to_entries();
        let proj = frame.projection();
        let clip = crate::ui_scene_options::transform(
            &crate::camera::multiply(&view, &proj),
            fine[0] as f32,
            y as f32,
            fine[1] as f32,
        );
        let mouse = [
            viewport[0] + ((clip[0] / clip[3] + 1.) * viewport[2] as f32 / 2.) as i32,
            viewport[1] + ((clip[1] / clip[3] + 1.) * viewport[3] as f32 / 2.) as i32,
        ];
        ui.engine.platform.mouse = mouse;
        ui.input.click = None;
        clock(&mut game, millis, |v| ui.tick(v))?;
        let entry = ui
            .state
            .minimenu
            .entries
            .iter()
            .map(|&id| ui.state.minimenu.entry(id))
            .find(|e| e.action == 60)
            .cloned()
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no Face here menu at {mouse:?} in {viewport:?}; options {:?}",
                    ui.input.scene_options
                )
            })?;
        assert_eq!([entry.tile_x, entry.tile_z], tile);
        ui.engine.outgoing.clear();
        ui.use_menu_option(&entry, mouse[0], mouse[1], false);
        assert_eq!(
            ui.engine.outgoing,
            std::fs::read(crate::test_support::replay_fixture(
                "navigation-menu",
                "face-click.bin"
            ))?
        );
        assert_eq!(
            ui.engine.menu.cross,
            Cross {
                x: mouse[0],
                y: mouse[1],
                mode: 1,
                cycle: 0
            }
        );
        let mut prioritised = entry.clone();
        prioritised.action += 2000;
        ui.engine.outgoing.clear();
        ui.use_menu_option(&prioritised, mouse[0], mouse[1], false);
        assert_eq!(
            ui.engine.outgoing,
            std::fs::read(crate::test_support::replay_fixture(
                "navigation-menu",
                "face-click.bin"
            ))?
        );
        proof.push(serde_json::json!({"canvas":canvas,"viewport":viewport,"mouse":mouse,"tile":tile,"wire":ui.engine.outgoing}));
    }
    assert!(!ui.diagnostics.executions.iter().any(|e| e["ok"] == false));
    std::fs::write(
        path.join("menu-proof.json"),
        serde_json::to_vec_pretty(&proof)?,
    )?;
    Ok(())
}

/// Exercises the production `Runtime::packet` route, including parser
/// output, retained player stats, and the 64-slot transmit ring.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn parsed_stat_frames_update_retained_owner_and_wrap_ring() -> anyhow::Result<()> {
    let pack = crate::test_support::require_pack("client.config.js5");
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    for step in 0..70u8 {
        let skill = step % 2;
        let xp = i32::from(step) * 1000;
        let mut wire = vec![0u8.wrapping_sub(step)];
        wire.extend_from_slice(&xp.to_le_bytes());
        wire.push(128u8.wrapping_sub(skill));
        let event = parse_ui_event(crate::proto::server::UPDATE_STAT, &wire)?.unwrap();
        with_game(&mut game, |vars| ui.packet(vars, &event))?;
    }
    let stats = game.ui_variables.stats.as_ref().unwrap();
    assert_eq!(stats.stat_xp_actual(0)?, 68_000);
    assert_eq!(stats.stat_xp_actual(1)?, 69_000);
    assert_eq!(game.ui_variables.stat_transmit.count, 70);
    assert_eq!(game.ui_variables.stat_transmit.ids[0], 0);
    assert_eq!(game.ui_variables.stat_transmit.ids[5], 1);
    assert_eq!(game.ui_variables.stat_transmit.ids[6], 0);
    game.ui_variables.stat_transmit.count = i32::MAX;
    let event = parse_ui_event(crate::proto::server::UPDATE_STAT, &[0, 1, 0, 0, 0, 128])?.unwrap();
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(game.ui_variables.stat_transmit.count, i32::MIN);
    assert_eq!(game.ui_variables.stat_transmit.ids[63], 0);
    with_game(&mut game, |vars| ui.packet(vars, &event))?;
    assert_eq!(game.ui_variables.stat_transmit.count, i32::MIN + 1);
    assert_eq!(game.ui_variables.stat_transmit.ids[0], 0);
    // A fresh production login creates new owners, discarding prior stats
    // and transmit history before any bootstrap packets are applied.
    let next = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    assert_eq!(next.ui_variables.stat_transmit.count, 0);
    assert_eq!(next.ui_variables.stat_transmit.ids, [0; 64]);
    assert_eq!(
        next.ui_variables
            .stats
            .as_ref()
            .unwrap()
            .stat_xp_actual(0)?,
        0
    );
    assert_eq!(next.ui_variables.stats.as_ref().unwrap().stat_level(0)?, 0);
    Ok(())
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn invalid_parsed_stat_leaves_retained_state_and_transmit_unchanged() -> anyhow::Result<()> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let valid = parse_ui_event(
        crate::proto::server::UPDATE_STAT,
        &[0u8.wrapping_sub(0), 1, 0, 0, 0, 128],
    )
    .unwrap()
    .unwrap();
    with_game(&mut game, |vars| ui.packet(vars, &valid))?;
    let before_count = game.ui_variables.stat_transmit.count;
    let before_ids = game.ui_variables.stat_transmit.ids;
    let invalid = parse_ui_event(crate::proto::server::UPDATE_STAT, &[0, 0, 0, 0, 0, 255])
        .unwrap()
        .unwrap();
    // Skill 255 is out of range (an out-of-range index is an error).
    assert!(with_game(&mut game, |vars| ui.packet(vars, &invalid)).is_err());
    assert_eq!(game.ui_variables.stat_transmit.count, before_count);
    assert_eq!(game.ui_variables.stat_transmit.ids, before_ids);
    assert_eq!(
        game.ui_variables
            .stats
            .as_ref()
            .unwrap()
            .stat_xp_actual(0)?,
        1
    );
    Ok(())
}

/// Ten thousand recorded `UPDATE_STAT` frames against the frozen recording of
/// the original client's stat class: XP, levels and the transmit ring after
/// each frame.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn stat_feed_matches_the_recording() -> anyhow::Result<()> {
    use rs910_core::test_support::frozen;
    let frames = frozen::bytes("stat-feed/frames.bin");
    let csv = frozen::text("stat-feed/expected.csv");
    let rows: Vec<Vec<i64>> = csv
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            line.split(',')
                .map(|v| v.parse::<i64>())
                .collect::<Result<_, _>>()
        })
        .collect::<Result<_, _>>()?;
    anyhow::ensure!(
        frames.len() == rows.len() * 7,
        "oracle frame/row count mismatch"
    );
    anyhow::ensure!(rows.iter().all(|r| r.len() == 9), "oracle column count");

    let pack = Pack::open(rs910_core::test_support::pack_root());
    let defaults_bytes = pack.read_group("defaults", 9)?.remove(&0);
    let defaults = crate::ui_stats::SkillDefaults::decode(defaults_bytes.as_deref())?;
    let mut ui = Runtime::new(pack.clone())?;
    ui.resize([800, 600])?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    game.ui_variables.stats = Some(crate::ui_stats::PlayerStats::new(defaults));
    game.ui_variables.stats.as_mut().unwrap().reset_session()?;
    for (step, row) in rows.iter().enumerate() {
        let frame = &frames[step * 7..step * 7 + 7];
        anyhow::ensure!(
            frame[0] == crate::proto::server::UPDATE_STAT,
            "oracle opcode at {step}"
        );
        let event = parse_ui_event(frame[0], &frame[1..])?
            .ok_or_else(|| anyhow::anyhow!("oracle stat event missing"))?;
        with_game(&mut game, |vars| ui.packet(vars, &event))?;
        let (_, skill, xp, current, xp_level, free_xp, free_level, members_xp, members_level) = (
            row[0], row[1], row[2], row[3], row[4], row[5], row[6], row[7], row[8],
        );
        anyhow::ensure!(row[0] == step as i64, "oracle step row {step}");
        let stats = game.ui_variables.stats.as_mut().unwrap();
        anyhow::ensure!(skill >= 0 && skill <= u8::MAX as i64, "oracle skill");
        anyhow::ensure!(
            stats.stat_xp_actual(skill as i32)? as i64 == xp,
            "xp row {step}"
        );
        anyhow::ensure!(
            stats.stat_level(skill as i32)? as i64 == current,
            "level row {step}"
        );
        anyhow::ensure!(
            stats.stat_level_max_actual(skill as i32)? as i64 == xp_level,
            "xp level row {step}"
        );
        stats.logged_in_members = false;
        anyhow::ensure!(
            stats.stat_xp(skill as i32)? as i64 == free_xp,
            "free xp row {step}"
        );
        anyhow::ensure!(
            stats.stat_level_max(skill as i32)? as i64 == free_level,
            "free level row {step}"
        );
        stats.logged_in_members = true;
        anyhow::ensure!(
            stats.stat_xp(skill as i32)? as i64 == members_xp,
            "members xp row {step}"
        );
        anyhow::ensure!(
            stats.stat_level_max(skill as i32)? as i64 == members_level,
            "members level row {step}"
        );
        anyhow::ensure!(
            game.ui_variables.stat_transmit.count == (step + 1) as i32,
            "oracle transmit count row {step}"
        );
        anyhow::ensure!(
            game.ui_variables.stat_transmit.ids[step & 63] == skill as i32,
            "oracle transmit row {step}"
        );
    }
    let tx = &game.ui_variables.stat_transmit;
    anyhow::ensure!(tx.count == rows.len() as i32, "oracle transmit count");
    let first = rows.len().saturating_sub(64);
    for (i, row) in rows.iter().enumerate().skip(first) {
        anyhow::ensure!(
            tx.ids[i & 63] == row[1] as i32,
            "oracle transmit ring slot {i}"
        );
    }
    Ok(())
}
