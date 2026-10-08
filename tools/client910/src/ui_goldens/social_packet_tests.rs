//! The social packet families against a recording of the original client.
//!
//! `fixtures/recorded/social-packets/input` is what the development server
//! sends one player through a scripted two-player session (friend and ignore
//! lists, chat filters, presence, private chat, friends chat, clan channel and
//! quick chat with every kind of dynamic value), plus frames a server could
//! send that it does not. The recording is the original client's social state
//! after each frame; the retained interface replays the same frames through
//! its ordinary packet path and must print the same state. Quick chat text is
//! cache content and is recorded as a digest.
use crate::client_game::with_game;
use crate::ui_runtime::*;
use crate::{cache::Pack, protocol910::live::Feed, session::parse_ui_event};
use native910::{
    script::Operand,
    vm::{Host, InstructionContext, Value},
};

fn digest(text: &str) -> String {
    let hash = text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    format!("{hash:016x}")
}

fn text(value: &str) -> String {
    if value.is_empty() {
        String::new()
    } else {
        value.to_owned()
    }
}

/// The state lines the recording prints after a frame, in its order.
fn dump(
    ui: &Runtime,
    game: &crate::client_game::ClientGame,
    speakers: &[usize],
    shown_history: &mut i32,
) -> Vec<String> {
    let social = &ui.engine.social;
    let mut out = vec![format!(
        "friends state={} count={}",
        social.friends_list_state,
        social.friends.len()
    )];
    for (i, f) in social.friends.iter().enumerate() {
        out.push(format!(
            " friend {i} name={} prev={} world={} wname={} rank={} platform={} referrer={} referred={} notes={} wflags={}",
            f.display_name, f.previous_name, f.world_id, f.world_name, f.rank, f.platform, f.referrer, f.referred, f.notes, f.world_flags
        ));
    }
    out.push(format!("ignores count={}", social.ignores.len()));
    for (i, g) in social.ignores.iter().enumerate() {
        out.push(format!(
            " ignore {i} nameU={} name={} notes={} temp={}",
            g.name_unfiltered, g.name, g.notes, g.temporary
        ));
    }
    let filters = &ui.engine.messages.filters;
    out.push(format!(
        "filters public={} private={} trade={}",
        filters[0].unwrap_or(-1),
        filters[1].unwrap_or(-1),
        filters[2].unwrap_or(-1)
    ));
    let chat = &social.friend_chat;
    out.push(format!(
        "friendchat owner={} display={} minkick={} rank={} count={}",
        chat.owner_name.as_deref().unwrap_or("null"),
        chat.display_name.as_deref().unwrap_or("null"),
        chat.min_kick,
        chat.rank,
        chat.users.len()
    ));
    for (i, u) in chat.users.iter().enumerate() {
        out.push(format!(
            " fcuser {i} name={} nameU={} world={} rank={} wname={}",
            u.name, u.name_unfiltered, u.world, u.rank, u.world_name
        ));
    }
    for (tag, channel) in [
        ("affined", social.affined_channel.as_ref()),
        ("listened", social.listened_channel.as_ref()),
    ] {
        let Some(c) = channel else {
            out.push(format!("clan {tag} null"));
            continue;
        };
        // The original keeps a missing clan name as null; the port folds it into the empty name.
        let clan_name = if c.clan_name.is_empty() {
            "null"
        } else {
            c.clan_name.as_str()
        };
        out.push(format!(
            "clan {tag} name={} node={} update={} talk={} kick={} users={} hashes={} displaynames={}",
            clan_name, c.node_id, c.update_num, c.rank_talk, c.rank_kick, c.users.len(), c.use_user_hashes, c.use_display_names
        ));
        for (i, u) in c.users.iter().enumerate() {
            out.push(format!(
                " cuser {i} name={} rank={} world={}",
                u.name, u.rank, u.world
            ));
        }
    }
    for (tag, settings) in [
        ("affined", social.affined_settings.as_ref()),
        ("listened", social.listened_settings.as_ref()),
    ] {
        dump_settings(&mut out, tag, settings);
    }
    dump_group(&mut out, social.player_group.as_ref());
    for &uid in speakers {
        let chat = game.runtime.feed.state.players.players[uid]
            .as_ref()
            .and_then(|player| player.chat.as_ref());
        if let Some(chat) = chat.filter(|chat| chat.text.is_some()) {
            out.push(format!(
                "speech {uid} text={} colour={} effect={}",
                chat.text.as_deref().unwrap_or(""),
                chat.colour,
                chat.effect
            ));
        }
    }
    let history = &ui.engine.messages.history;
    let last = history.last_uid();
    for uid in *shown_history..=last {
        let Some(l) = history.get_by_uid(uid) else {
            continue;
        };
        let message = if l.phrase >= 0 {
            format!("#{}", digest(&l.message))
        } else {
            text(&l.message)
        };
        out.push(format!(
            "chat uid={} type={} flags={} name={} nameU={} nameS={} clan={} phrase={} message={} crown={}",
            l.uid,
            l.chat_type,
            l.flags,
            l.name,
            l.name_unfiltered,
            l.name_simple,
            l.clan.as_deref().unwrap_or("null"),
            l.phrase,
            message,
            l.crown.map_or("null".into(), |c| c.to_string())
        ));
    }
    *shown_history = last.wrapping_add(1);
    out
}

fn sparse(value: &crate::ui_vars::Value) -> String {
    match value {
        crate::ui_vars::Value::Int(v) => v.to_string(),
        crate::ui_vars::Value::Long(v) => format!("L{v}"),
        crate::ui_vars::Value::String(v) => format!("S{}", String::from_utf16_lossy(v)),
        other => format!("?{other:?}"),
    }
}

fn sparse_map(map: &std::collections::BTreeMap<i32, crate::ui_vars::Value>) -> String {
    map.iter()
        .map(|(id, v)| format!("{id}:{}", sparse(v)))
        .collect::<Vec<_>>()
        .join(",")
}

fn dump_settings(
    out: &mut Vec<String>,
    tag: &str,
    settings: Option<&crate::ui_social::ClanSettingsState>,
) {
    let Some(c) = settings else {
        out.push(format!("settings {tag} null"));
        return;
    };
    out.push(format!(
        "settings {tag} name={} owner={} update={} allowUnaffined={} talk={} kick={} lootshare={} coinshare={} members={} banned={} owner_slot={} replacement={} hashes={} displaynames={}",
        c.clan_name, c.owner, c.update_num, c.allow_unaffined, c.rank_talk, c.rank_kick, c.rank_lootshare, c.coinshare, c.members.len(), c.banned.len(), c.current_owner_slot, c.replacement_owner_slot, c.use_user_hashes, c.use_display_names
    ));
    for (i, m) in c.members.iter().enumerate() {
        let hash = if c.use_user_hashes {
            m.hash.to_string()
        } else {
            "-".into()
        };
        let name = if c.use_display_names {
            m.display_name.clone()
        } else {
            "null".into()
        };
        out.push(format!(
            " smember {i} hash={hash} name={name} rank={} extra={} joined={} muted={}",
            m.rank, m.extra, m.joined_runedays, m.muted
        ));
    }
    for (i, name) in c.banned.iter().enumerate() {
        out.push(format!(" sbanned {i} hash=- name={name}"));
    }
    let mut settings = Vec::new();
    for uid in 0..16 {
        match c.settings.get(&uid) {
            Some(crate::ui_social::ClanSettingValue::Int(v)) => {
                settings.push(format!("{uid}=i{v}"))
            }
            Some(crate::ui_social::ClanSettingValue::Long(v)) => {
                settings.push(format!("{uid}=L{v}"))
            }
            Some(crate::ui_social::ClanSettingValue::String(v)) => {
                settings.push(format!("{uid}=S{v}"))
            }
            None => {}
        }
    }
    out.push(format!(" settings {}", settings.join(" ")));
}

fn dump_group(out: &mut Vec<String>, group: Option<&crate::ui_social::PlayerGroupState>) {
    let Some(g) = group else {
        out.push("group null".into());
        return;
    };
    out.push(format!(
        "group name={} hash={} membersOnly={} max={} hasUid={} hasDisplayName={} owner={} update={} created={} members={} banned={} vars={}",
        g.display_name, g.hashcode, g.members_only, g.max_size, g.has_uid, g.has_display_name, g.owner_slot, g.update_num, g.creation_time, g.members.len(), g.banned.len(), sparse_map(&g.vars)
    ));
    for (i, m) in g.members.iter().enumerate() {
        let xp = m
            .stats
            .iter()
            .enumerate()
            .filter(|(_, v)| **v != 0)
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(",");
        let variables = m.variables.as_ref().map_or("null".into(), sparse_map);
        out.push(format!(
            " gmember {i} name={} uid={} rank={} status={} team={} online={} members={} node={} xp={xp} vars={} variables={variables}",
            m.display_name, m.group_uid, m.rank, m.status, m.team, m.online, m.members, m.node_id, sparse_map(&m.vars)
        ));
    }
    for (i, name) in g.banned.iter().enumerate() {
        out.push(format!(" gbanned {i} name={name}"));
    }
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

/// Replays the corpus and returns the printed state, in the recording's format.
fn replay(input: &str) -> anyhow::Result<String> {
    let pack = Pack::open(rs910_core::test_support::pack_root());
    let mut ui = Runtime::new(pack.clone())?;
    let mut game = crate::client_game::ClientGame::login(&pack, 1, Feed::default(), 910, false)?;
    let mut out = Vec::new();
    let mut shown = 0;
    let mut n = 0;
    let mut speakers: Vec<usize> = Vec::new();
    for line in input.lines() {
        let mut parts = line.splitn(3, ' ');
        match parts.next() {
            Some("set") => match (parts.next(), parts.next()) {
                (Some("local"), Some(name)) => ui.engine.login.lobby_player_name = name.into(),
                (Some("world"), Some(node)) => ui.engine.login.world = node.parse()?,
                (Some("player"), Some(spec)) => {
                    // A player in view with a body, as `MESSAGE_PUBLIC` needs.
                    let (uid, name) = spec.split_once(' ').unwrap();
                    let uid: usize = uid.parse()?;
                    let mut player = crate::entities910::Player::default();
                    player.appearance.name = Some(name.into());
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
                    game.runtime.feed.state.players.players[uid] = Some(player);
                    speakers.push(uid);
                }
                (Some("flags"), Some(flags)) => {
                    let f: Vec<&str> = flags.split(' ').collect();
                    ui.engine.account.dob_verified = f[0] == "1";
                    ui.engine.account.player_is_quickchat = f[1] == "1";
                    ui.engine.account.logged_in_quickchat = f[2] == "1";
                }
                other => anyhow::bail!("unknown set directive {other:?}"),
            },
            Some("cmd") => {
                let name = parts.next().unwrap();
                let args = parts.next().unwrap_or("- -");
                let (int_text, obj_text) = args.split_once(' ').unwrap_or(("-", "-"));
                let mut ints: Vec<i32> = if int_text == "-" {
                    Vec::new()
                } else {
                    int_text
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?
                };
                let mut objs: Vec<String> = if obj_text == "-" {
                    Vec::new()
                } else {
                    obj_text.split('|').map(String::from).collect()
                };
                let mut longs = Vec::new();
                let operand = Operand::Byte(0);
                let context = InstructionContext {
                    script_name: Some("social-oracle"),
                    script_id: None,
                    event: None,
                    pc: 0,
                    command: name,
                    operand: &operand,
                    secondary: false,
                    int_locals: &[],
                };
                ui.engine.outgoing.clear();
                let result = ui
                    .engine
                    .trap_context(&context, &mut ints, &mut objs, &mut longs);
                let thrown = result.is_err();
                match result {
                    Ok(Some(Value::Int(v))) => ints.push(v),
                    Ok(Some(Value::Str(v))) => objs.push(v),
                    Ok(Some(Value::Long(v))) => longs.push(v),
                    _ => {}
                }
                // Lines a command adds reach the history at once, as in the original.
                for (kind, message) in std::mem::take(&mut ui.engine.social.system_messages) {
                    ui.engine.messages.history.add_system_message(kind, message);
                }
                let int_list = ints
                    .iter()
                    .map(i32::to_string)
                    .collect::<Vec<_>>()
                    .join(",");
                if thrown {
                    // A command that fails leaves the stacks half-written; only the failure is recorded.
                    out.push(format!("cmd {name} thrown=1"));
                } else {
                    // The history line time is the wall clock.
                    if name.starts_with("chat_gethistory") && !objs.is_empty() {
                        objs[0] = "<time>".into();
                    }
                    out.push(format!(
                        "cmd {name} ints=[{int_list}] objs=[{}] thrown=0 out={}",
                        objs.join("|"),
                        ui.engine
                            .outgoing
                            .iter()
                            .map(|b| format!("{b:02x}"))
                            .collect::<String>()
                    ));
                }
                let history = &ui.engine.messages.history;
                for uid in shown..=history.last_uid() {
                    if let Some(line) = history.get_by_uid(uid) {
                        out.push(format!(
                            "chat uid={} type={} message={}",
                            line.uid, line.chat_type, line.message
                        ));
                    }
                }
                shown = history.last_uid().wrapping_add(1);
            }
            Some("packet") => {
                n += 1;
                let opcode: u8 = parts.next().unwrap().parse()?;
                let hex = parts.next().unwrap_or("");
                let payload = unhex(hex);
                out.push(format!("packet {n} {opcode} {hex}"));
                let event = parse_ui_event(opcode, &payload)?;
                if let Some(event) = event {
                    with_game(&mut game, |vars| ui.packet(vars, &event))?;
                }
                crate::app::apply_overhead_chat(&mut game, &mut ui.engine.effects.overhead_chat);
                out.extend(dump(&ui, &game, &speakers, &mut shown));
            }
            _ => {}
        }
    }
    Ok(out.join("\n") + "\n")
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn social_packets_match_the_recorded_client_state() -> anyhow::Result<()> {
    let input = rs910_core::test_support::frozen::text("social-packets/input");
    let actual = replay(&input)?;
    let recorded = rs910_core::test_support::frozen::text("social-packets/recording");
    if actual != recorded {
        // Keep the printed state for a side-by-side diff; name the first differing line.
        let saved = std::env::temp_dir().join("social-packets-actual.txt");
        let _ = std::fs::write(&saved, &actual);
        let line = actual
            .lines()
            .zip(recorded.lines())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.lines().count().min(recorded.lines().count()));
        anyhow::bail!(
            "social state differs at line {} (state written to {}): got {:?}, recorded {:?}",
            line + 1,
            saved.display(),
            actual.lines().nth(line),
            recorded.lines().nth(line)
        );
    }
    Ok(())
}
