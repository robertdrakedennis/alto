//! Chat filters and the message packets (public, private, game, quick chat,
//! friend/clan/player-group channels).
//!
//! Applier of the packet router ([`super`]); each arm is the former
//! `packet_event` branch for its variants, moved verbatim in order.
use super::super::crown_image;
use super::super::CrownedLine;
use super::super::OverheadChat;
use super::super::Runtime;
use crate::ui_chat::NewChatLine;
use crate::ui_vars::Variables;
use anyhow::Result;

impl Runtime {
    pub(super) fn apply_chat_packet(
        &mut self,
        vars: &mut Variables<'_>,
        event: &crate::server_prot::UiEvent,
    ) -> Result<()> {
        match event {
            crate::server_prot::UiEvent::ChatFilters { trade, public } => {
                self.engine.messages.filters[0] = Some(*public);
                self.engine.messages.filters[2] = Some(*trade);
                Ok(())
            }
            crate::server_prot::UiEvent::ChatPrivateFilter { value } => {
                self.engine.messages.filters[1] = *value;
                Ok(())
            }
            crate::server_prot::UiEvent::FriendChannelMessage { bytes } => {
                // read.
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let has_unfiltered = reader.g1()? == 1;
                let name = reader.gjstr()?;
                let name_unfiltered = if has_unfiltered {
                    reader.gjstr()?
                } else {
                    name.clone()
                };
                let clan = reader.gjstr()?;
                let high = i64::from(reader.g2()?);
                let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                let crown = reader.g1()?;
                let message_id = (high << 32) + low;
                if self.engine.messages.message_seen(message_id)
                    || self.engine.drop_free_text(crown, &name_unfiltered)
                {
                    return Ok(());
                }
                let message = crate::wordpack::escape(crate::wordpack::decode_string(
                    huffman,
                    bytes,
                    &mut reader.pos,
                )?);
                reader.finish("MESSAGE_FRIENDCHANNEL")?;
                self.engine.messages.record_message(message_id);
                self.engine.messages.add_crowned_line(CrownedLine {
                    chat_type: 9,
                    name: &name,
                    name_unfiltered: &name_unfiltered,
                    name_simple: name.clone(),
                    clan: Some(clan),
                    phrase: -1,
                    message,
                    crown,
                });
                Ok(())
            }
            crate::server_prot::UiEvent::ClanChannelMessage { bytes } => {
                // read.
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let affined = reader.g1()? == 1;
                let name = reader.gjstr()?;
                let high = i64::from(reader.g2()?);
                let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                let crown = reader.g1()?;
                let message_id = (high << 32) + low;
                let clan = if affined {
                    self.engine.social.affined_channel.as_ref()
                } else {
                    self.engine.social.listened_channel.as_ref()
                };
                let Some(clan_name) = clan.map(|clan| clan.clan_name.clone()) else {
                    return Ok(());
                };
                if self.engine.messages.message_seen(message_id)
                    || self.engine.drop_free_text(crown, &name)
                {
                    return Ok(());
                }
                let message = crate::wordpack::escape(crate::wordpack::decode_string(
                    huffman,
                    bytes,
                    &mut reader.pos,
                )?);
                reader.finish("MESSAGE_CLANCHANNEL")?;
                self.engine.messages.record_message(message_id);
                self.engine.messages.add_crowned_line(CrownedLine {
                    chat_type: if affined { 41 } else { 44 },
                    name: &name,
                    name_unfiltered: &name,
                    name_simple: name.clone(),
                    clan: Some(clan_name),
                    phrase: -1,
                    message,
                    crown,
                });
                Ok(())
            }
            crate::server_prot::UiEvent::ClanChannelSystemMessage { bytes } => {
                // read: no escape, no crown.
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let affined = reader.g1()? == 1;
                let high = i64::from(reader.g2()?);
                let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                let message_id = (high << 32) + low;
                let clan = if affined {
                    self.engine.social.affined_channel.as_ref()
                } else {
                    self.engine.social.listened_channel.as_ref()
                };
                let Some(clan_name) = clan.map(|clan| clan.clan_name.clone()) else {
                    return Ok(());
                };
                if self.engine.messages.message_seen(message_id) {
                    return Ok(());
                }
                let message = crate::wordpack::decode_string(huffman, bytes, &mut reader.pos)?;
                reader.finish("MESSAGE_CLANCHANNEL_SYSTEM")?;
                self.engine.messages.record_message(message_id);
                self.engine.messages.history.add_message(NewChatLine {
                    clan: Some(clan_name),
                    ..NewChatLine::system(if affined { 43 } else { 46 }, message)
                });
                self.engine.messages.changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::PlayerGroupMessage { bytes } => {
                // read.
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let name = reader.gjstr()?;
                let high = i64::from(reader.g2()?);
                let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                let crown = reader.g1()?;
                let quickchat = reader.g1()? == 1;
                let message_id = (high << 32) + low;
                if !self.engine.social.player_group_present
                    || self.engine.messages.message_seen(message_id)
                    || self.engine.drop_free_text(crown, &name)
                {
                    return Ok(());
                }
                let message = crate::wordpack::escape(crate::wordpack::decode_string(
                    huffman,
                    bytes,
                    &mut reader.pos,
                )?);
                reader.finish("MESSAGE_PLAYER_GROUP")?;
                self.engine.messages.record_message(message_id);
                let group = self.engine.social.player_group_name.clone();
                self.engine.messages.add_crowned_line(CrownedLine {
                    chat_type: if quickchat { 22 } else { 24 },
                    name: &name,
                    name_unfiltered: &name,
                    name_simple: name.clone(),
                    clan: Some(group),
                    phrase: -1,
                    message,
                    crown,
                });
                Ok(())
            }
            crate::server_prot::UiEvent::QuickChat { opcode, bytes } => {
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                match *opcode {
                    crate::proto::server::MESSAGE_QUICKCHAT_PRIVATE => {
                        // read.
                        let has_unfiltered = reader.g1()? == 1;
                        let name = reader.gjstr()?;
                        let name_unfiltered = if has_unfiltered {
                            reader.gjstr()?
                        } else {
                            name.clone()
                        };
                        let high = i64::from(reader.g2()?);
                        let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                        let crown = reader.g1()?;
                        let phrase_id = reader.g2()?;
                        let message_id = (high << 32) + low;
                        if self.engine.messages.message_seen(message_id)
                            || self.engine.drop_quick_chat(crown, &name_unfiltered)
                        {
                            return Ok(());
                        }
                        self.engine.messages.record_message(message_id);
                        let message = self.render_quickchat(phrase_id, bytes, &mut reader.pos);
                        reader.finish("MESSAGE_QUICKCHAT_PRIVATE")?;
                        self.engine.messages.add_crowned_line(CrownedLine {
                            chat_type: 18,
                            name: &name,
                            name_unfiltered: &name_unfiltered,
                            name_simple: name.clone(),
                            clan: None,
                            phrase: i32::from(phrase_id),
                            message,
                            crown,
                        });
                    }
                    crate::proto::server::MESSAGE_QUICKCHAT_PRIVATE_ECHO => {
                        // read: no ignore test, null crown.
                        let name = reader.gjstr()?;
                        let phrase_id = reader.g2()?;
                        let message = self.render_quickchat(phrase_id, bytes, &mut reader.pos);
                        reader.finish("MESSAGE_QUICKCHAT_PRIVATE_ECHO")?;
                        self.engine.messages.history.add_message(NewChatLine {
                            phrase: i32::from(phrase_id),
                            ..NewChatLine::from_sender(19, name, message)
                        });
                        self.engine.messages.changed = true;
                    }
                    crate::proto::server::MESSAGE_QUICKCHAT_FRIENDCHAT => {
                        // read.
                        let has_unfiltered = reader.g1()? == 1;
                        let name = reader.gjstr()?;
                        let name_unfiltered = if has_unfiltered {
                            reader.gjstr()?
                        } else {
                            name.clone()
                        };
                        let clan = reader.gjstr()?;
                        let high = i64::from(reader.g2()?);
                        let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                        let crown = reader.g1()?;
                        let phrase_id = reader.g2()?;
                        let message_id = (high << 32) + low;
                        if self.engine.messages.message_seen(message_id)
                            || self.engine.drop_quick_chat(crown, &name_unfiltered)
                        {
                            return Ok(());
                        }
                        self.engine.messages.record_message(message_id);
                        let message = self.render_quickchat(phrase_id, bytes, &mut reader.pos);
                        reader.finish("MESSAGE_QUICKCHAT_FRIENDCHAT")?;
                        self.engine.messages.add_crowned_line(CrownedLine {
                            chat_type: 20,
                            name: &name,
                            name_unfiltered: &name_unfiltered,
                            name_simple: name.clone(),
                            clan: Some(clan),
                            phrase: i32::from(phrase_id),
                            message,
                            crown,
                        });
                    }
                    crate::proto::server::MESSAGE_QUICKCHAT_CLANCHANNEL => {
                        // read.
                        let affined = reader.g1()? == 1;
                        let name = reader.gjstr()?;
                        let high = i64::from(reader.g2()?);
                        let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                        let crown = reader.g1()?;
                        let phrase_id = reader.g2()?;
                        let message_id = (high << 32) + low;
                        let channel = if affined {
                            self.engine.social.affined_channel.as_ref()
                        } else {
                            self.engine.social.listened_channel.as_ref()
                        };
                        let Some(clan) = channel.map(|channel| channel.clan_name.clone()) else {
                            return Ok(());
                        };
                        if self.engine.messages.message_seen(message_id)
                            || self.engine.drop_quick_chat(crown, &name)
                        {
                            return Ok(());
                        }
                        self.engine.messages.record_message(message_id);
                        let message = self.render_quickchat(phrase_id, bytes, &mut reader.pos);
                        reader.finish("MESSAGE_QUICKCHAT_CLANCHANNEL")?;
                        self.engine.messages.add_crowned_line(CrownedLine {
                            chat_type: if affined { 42 } else { 45 },
                            name: &name,
                            name_unfiltered: &name,
                            name_simple: name.clone(),
                            clan: Some(clan),
                            phrase: i32::from(phrase_id),
                            message,
                            crown,
                        });
                    }
                    crate::proto::server::MESSAGE_QUICKCHAT_PLAYER_GROUP => {
                        // read.
                        let name = reader.gjstr()?;
                        let high = i64::from(reader.g2()?);
                        let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                        let crown = reader.g1()?;
                        let quickchat = reader.g1()? == 1;
                        let phrase_id = reader.g2()?;
                        let message_id = (high << 32) + low;
                        if !self.engine.social.player_group_present
                            || self.engine.messages.message_seen(message_id)
                            || self.engine.drop_quick_chat(crown, &name)
                        {
                            return Ok(());
                        }
                        self.engine.messages.record_message(message_id);
                        let group_name = self.engine.social.player_group_name.clone();
                        let message = self.render_quickchat(phrase_id, bytes, &mut reader.pos);
                        reader.finish("MESSAGE_QUICKCHAT_PLAYER_GROUP")?;
                        self.engine.messages.add_crowned_line(CrownedLine {
                            chat_type: if quickchat { 23 } else { 25 },
                            name: &name,
                            name_unfiltered: &name,
                            name_simple: name.clone(),
                            clan: Some(group_name),
                            phrase: i32::from(phrase_id),
                            message,
                            crown,
                        });
                    }
                    _ => return Ok(()),
                }
                Ok(())
            }
            crate::server_prot::UiEvent::PublicMessage { bytes } => {
                // read.
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let player_id = usize::from(reader.g2()?);
                let mut raw_flags = i32::from(reader.g2()?);
                let crown = i32::from(reader.g1()?);
                let quickchat = raw_flags & 0x8000 != 0;
                if crate::ui_debug_flags::flags().chat_trace {
                    let player = vars
                        .scene
                        .players
                        .and_then(|players| players.players.get(player_id))
                        .and_then(Option::as_ref);
                    log::info!(
                    "[chat-trace] MESSAGE_PUBLIC player={player_id} flags={raw_flags:#x} crown={crown} present={} name={:?} model={}",
                    player.is_some(),
                    player.and_then(|p| p.appearance.name.clone()),
                    player.is_some_and(|p| p.appearance.model.is_some())
                );
                }
                let Some(player) = vars
                    .scene
                    .players
                    .and_then(|players| players.players.get(player_id))
                    .and_then(Option::as_ref)
                else {
                    return Ok(());
                };
                //  `nameUnfiltered != null && model != null`.
                let (Some(raw_name), true) = (
                    player.appearance.name.as_ref(),
                    player.appearance.model.is_some(),
                ) else {
                    return Ok(());
                };
                // Only STAFF_MOD (2) is not ignorable.
                let ignorable = crown != 2;
                if ignorable
                    && ((!quickchat
                        && (self.engine.account.dob_verified
                            && !self.engine.account.player_is_quickchat
                            || self.engine.account.logged_in_quickchat))
                        || self.engine.social.ignore_test(raw_name))
                {
                    return Ok(());
                }
                let titled_name = player.appearance.title.as_ref().map_or_else(
                    || raw_name.clone(),
                    |title| title.replace("<name>", raw_name),
                );
                let phrase_id = if quickchat {
                    raw_flags &= 0x7fff;
                    i32::from(reader.g2()?)
                } else {
                    -1
                };
                let message = if quickchat {
                    self.render_quickchat(phrase_id as u16, bytes, &mut reader.pos)
                } else {
                    crate::wordpack::escape(crate::wordpack::decode_string(
                        self.engine
                            .configs
                            .wordpack
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?,
                        bytes,
                        &mut reader.pos,
                    )?)
                };
                reader.finish("MESSAGE_PUBLIC")?;
                // The line is the trimmed text with the colour (`flags >> 8`) and
                // effect (`flags & 0xFF`).
                self.engine.effects.overhead_chat.push(OverheadChat {
                    player: player_id,
                    text: message.trim_matches(|c: char| c <= ' ').to_string(),
                    colour: raw_flags >> 8,
                    effect: raw_flags & 0xff,
                });
                let image = crown_image(crown);
                let display_name = image.map_or_else(
                    || titled_name.clone(),
                    |img| format!("<img={img}>{titled_name}"),
                );
                let display_unfiltered =
                    image.map_or_else(|| raw_name.clone(), |img| format!("<img={img}>{raw_name}"));
                let chat_type = if quickchat {
                    17
                } else if matches!(crown, 1 | 2 | 3 | 5) {
                    1
                } else {
                    2
                };
                if crate::ui_debug_flags::flags().chat_trace {
                    log::info!(
                    "[chat-trace] public line type={chat_type} text={message:?} overhead_queue={}",
                    self.engine.effects.overhead_chat.len()
                );
                }
                //  addChatLine(type, 0, ...): the public
                // chat flags feed only the overhead line, never the history row.
                self.engine.messages.history.add_message(NewChatLine {
                    name: display_name,
                    name_unfiltered: display_unfiltered,
                    name_simple: raw_name.clone(),
                    phrase: phrase_id,
                    crown: Some(crown),
                    ..NewChatLine::system(chat_type, message)
                });
                self.engine.messages.changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::GameMessage {
                chat_type,
                flags,
                name,
                name_unfiltered,
                message,
            } => {
                // read. Developer-console types are consumed by
                // the console owner; all regular types enter ChatHistory exactly
                // as addMessage does in the original client.
                if *chat_type == 99 {
                    self.engine.effects.console_messages.push(message.clone());
                } else if *chat_type == 98 {
                    // Replace the console input line.
                    self.engine.builtins.requests.push(
                        crate::ui_runtime::host_builtins::Request::ConsoleEntry(message.clone()),
                    );
                } else if !name_unfiltered.is_empty()
                    && self.engine.social.ignore_test(name_unfiltered)
                {
                    // read drops ignored senders.
                } else {
                    self.engine.messages.history.add_message(NewChatLine {
                        flags: *flags,
                        name: name.clone(),
                        name_unfiltered: name_unfiltered.clone(),
                        name_simple: name.clone(),
                        ..NewChatLine::system(*chat_type, message.clone())
                    });
                    self.engine.messages.changed = true;
                }
                Ok(())
            }
            crate::server_prot::UiEvent::PrivateMessageEcho { bytes } => {
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut pos = 0usize;
                let sender = crate::wordpack::read_gjstr(bytes, &mut pos)?;
                let message = crate::wordpack::escape(crate::wordpack::decode_string(
                    huffman, bytes, &mut pos,
                )?);
                anyhow::ensure!(pos == bytes.len(), "MESSAGE_PRIVATE_ECHO trailing bytes");
                self.engine
                    .messages
                    .history
                    .add_message(NewChatLine::from_sender(6, sender, message));
                self.engine.messages.changed = true;
                Ok(())
            }
            crate::server_prot::UiEvent::PrivateMessage { bytes } => {
                // read.
                let huffman = self
                    .engine
                    .configs
                    .wordpack
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("chat Huffman coder not loaded"))?;
                let mut reader = crate::server_prot::PayloadReader::new(bytes);
                let has_unfiltered = reader.g1()? == 1;
                let name = reader.gjstr()?;
                let name_unfiltered = if has_unfiltered {
                    reader.gjstr()?
                } else {
                    name.clone()
                };
                let high = i64::from(reader.g2()?);
                let low = i64::from(reader.g3s()? & 0x00ff_ffff);
                let crown = reader.g1()?;
                let message_id = (high << 32) + low;
                if self.engine.messages.message_seen(message_id)
                    || self.engine.drop_free_text(crown, &name_unfiltered)
                {
                    return Ok(());
                }
                let message = crate::wordpack::escape(crate::wordpack::decode_string(
                    huffman,
                    bytes,
                    &mut reader.pos,
                )?);
                reader.finish("MESSAGE_PRIVATE")?;
                self.engine.messages.record_message(message_id);
                // The moderator crowns (PLAYER_MOD, STAFF_MOD, LOCAL_MOD,
                // PREMIER_CLUB_PLAYER_MOD) select type 7.
                let chat_type = if matches!(crown, 1 | 2 | 3 | 5) { 7 } else { 3 };
                self.engine.messages.add_crowned_line(CrownedLine {
                    chat_type,
                    name: &name,
                    name_unfiltered: &name_unfiltered,
                    name_simple: name.clone(),
                    clan: None,
                    phrase: -1,
                    message,
                    crown,
                });
                Ok(())
            }
            other => anyhow::bail!("packet router: {other:?} is not a chat packet"),
        }
    }
}
