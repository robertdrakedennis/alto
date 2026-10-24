//! Social list mutations, validation and outgoing request packets.

use native910::vm::{VmError, VmResult};

use super::{friend_related, namespace_normalize, SocialText, State};

impl State {
    pub fn handles_mutation(command: &str) -> bool {
        matches!(
            command,
            "friend_add"
                | "friend_del"
                | "friend_setrank"
                | "friend_setnotes"
                | "ignore_add"
                | "ignore_add_temp"
                | "ignore_del"
                | "ignore_setnotes"
                | "clan_kickuser"
                | "clan_joinchat"
                | "clan_leavechat"
                | "activeclanchannel_kickuser"
        )
    }
    pub(super) fn pjstr(text: &str) -> Result<Vec<u8>, VmError> {
        let (_, payload) =
            crate::ui_dialogue::build_resume_name(text).ok_or_else(|| VmError::TrapFailed {
                command: "social".into(),
                reason: "social name/note contains NUL".into(),
            })?;
        Ok(payload[1..].to_vec())
    }
    pub(super) fn one_string_packet(
        opcode: u8,
        text: &str,
        extra_prefix: u8,
    ) -> Result<Vec<u8>, VmError> {
        let body = Self::pjstr(text)?;
        let prefix = body.len().wrapping_add(usize::from(extra_prefix));
        let mut packet = Vec::with_capacity(2 + body.len());
        packet.extend([opcode, prefix as u8]);
        packet.extend(body);
        Ok(packet)
    }
    pub(super) fn clan_channel_kick_packet(
        affined: bool,
        index: usize,
        name: &str,
    ) -> Result<Vec<u8>, VmError> {
        let body = Self::pjstr(name)?;
        let mut packet = Vec::with_capacity(6 + body.len());
        packet.extend([
            crate::proto::client::CLANCHANNEL_KICKUSER,
            body.len().wrapping_add(3) as u8,
            u8::from(affined),
            (index >> 8) as u8,
            index as u8,
        ]);
        packet.extend(body);
        Ok(packet)
    }
    pub(super) fn truncate_note(text: String) -> String {
        let units: Vec<u16> = text.encode_utf16().take(30).collect();
        String::from_utf16_lossy(&units)
    }
    /// Applies the local add-friend guards, then builds the packet.
    pub(super) fn add_friend(&mut self, name: &str) -> VmResult<Vec<u8>> {
        let limit = if self.player_is_members { 400 } else { 200 };
        if self.friends.len() >= limit {
            self.system_messages.push((
                4,
                if self.player_is_members {
                    SocialText::FriendListFullMembers
                } else {
                    SocialText::FriendListFull
                }
                .text()
                .to_string(),
            ));
            return Ok(Vec::new());
        }
        let Some(normal) = namespace_normalize(name) else {
            return Ok(Vec::new());
        };
        for friend in &self.friends {
            let dupe = namespace_normalize(&friend.display_name).as_deref() == Some(&normal)
                || (!friend.previous_name.is_empty()
                    && namespace_normalize(&friend.previous_name).as_deref() == Some(&normal));
            if dupe {
                self.system_messages
                    .push((4, format!("{name}{}", SocialText::FriendListDupe.text())));
                return Ok(Vec::new());
            }
        }
        for ignore in &self.ignores {
            let ignored = namespace_normalize(&ignore.name_unfiltered).as_deref() == Some(&normal)
                || (!ignore.name.is_empty()
                    && namespace_normalize(&ignore.name).as_deref() == Some(&normal));
            if ignored {
                self.system_messages.push((
                    4,
                    format!(
                        "{}{name}{}",
                        SocialText::RemoveIgnore1.text(),
                        SocialText::RemoveIgnore2.text()
                    ),
                ));
                return Ok(Vec::new());
            }
        }
        if namespace_normalize(&self.local_player_name).as_deref() == Some(&normal) {
            self.system_messages
                .push((4, SocialText::FriendCantAddSelf.text().to_string()));
            return Ok(Vec::new());
        }
        Self::one_string_packet(crate::proto::client::FRIENDLIST_ADD, name, 0)
    }
    /// Applies the local add-ignore guards, then builds the packet.
    pub(super) fn add_ignore(&mut self, name: &str, temporary: bool) -> VmResult<Vec<u8>> {
        let limit = if self.player_is_members { 400 } else { 100 };
        if self.ignores.len() >= limit {
            self.system_messages.push((
                4,
                if self.player_is_members {
                    SocialText::IgnoreListFullMembers
                } else {
                    SocialText::IgnoreListFull
                }
                .text()
                .to_string(),
            ));
            return Ok(Vec::new());
        }
        let Some(normal) = namespace_normalize(name) else {
            return Ok(Vec::new());
        };
        for ignore in &self.ignores {
            let dupe = namespace_normalize(&ignore.name_unfiltered).as_deref() == Some(&normal)
                || (!ignore.name.is_empty()
                    && namespace_normalize(&ignore.name).as_deref() == Some(&normal));
            if dupe {
                self.system_messages
                    .push((4, format!("{name}{}", SocialText::IgnoreListDupe.text())));
                return Ok(Vec::new());
            }
        }
        for friend in &self.friends {
            let listed = namespace_normalize(&friend.display_name).as_deref() == Some(&normal)
                || (!friend.previous_name.is_empty()
                    && namespace_normalize(&friend.previous_name).as_deref() == Some(&normal));
            if listed {
                self.system_messages.push((
                    4,
                    format!(
                        "{}{name}{}",
                        SocialText::RemoveFriend1.text(),
                        SocialText::RemoveFriend2.text()
                    ),
                ));
                return Ok(Vec::new());
            }
        }
        if namespace_normalize(&self.local_player_name).as_deref() == Some(&normal) {
            self.system_messages
                .push((4, SocialText::IgnoreCantAddSelf.text().to_string()));
            return Ok(Vec::new());
        }
        let mut packet = Self::one_string_packet(crate::proto::client::IGNORELIST_ADD, name, 1)?;
        packet.push(u8::from(temporary));
        Ok(packet)
    }
    /// Removes the first related display name, stamps the friend transmit,
    /// then sends.
    pub(super) fn del_friend(&mut self, name: &str) -> VmResult<Vec<u8>> {
        let Some(normal) = namespace_normalize(name) else {
            return Ok(Vec::new());
        };
        let Some(index) = self.friends.iter().position(|friend| {
            friend_related(
                name,
                &normal,
                &friend.display_name,
                namespace_normalize(&friend.display_name).as_deref(),
            )
        }) else {
            return Ok(Vec::new());
        };
        self.friends.remove(index);
        self.stamps.friend = true;
        Self::one_string_packet(crate::proto::client::FRIENDLIST_DEL, name, 0)
    }
    /// Removes the first related ignore entry, stamps the friend transmit, then sends.
    pub(super) fn del_ignore(&mut self, name: &str) -> VmResult<Vec<u8>> {
        let Some(normal) = namespace_normalize(name) else {
            return Ok(Vec::new());
        };
        let Some(index) = self.ignores.iter().position(|ignore| {
            friend_related(
                name,
                &normal,
                &ignore.name_unfiltered,
                namespace_normalize(&ignore.name_unfiltered).as_deref(),
            )
        }) else {
            return Ok(Vec::new());
        };
        self.ignores.remove(index);
        self.stamps.friend = true;
        Self::one_string_packet(crate::proto::client::IGNORELIST_DEL, name, 0)
    }
    /// Build a normal client-to-server social mutation and apply the
    /// immediate local deletion behavior. Add/rank/note changes are server
    /// authoritative and are reflected by the next incoming list update.
    pub fn mutation(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
    ) -> VmResult<Vec<u8>> {
        match command {
            "friend_add" => {
                let name = Self::pop_name(objs)?;
                self.add_friend(&name)
            }
            "friend_del" => {
                let name = Self::pop_name(objs)?;
                self.del_friend(&name)
            }
            "ignore_add" | "ignore_add_temp" => {
                let name = Self::pop_name(objs)?;
                self.add_ignore(&name, command == "ignore_add_temp")
            }
            "ignore_del" => {
                let name = Self::pop_name(objs)?;
                self.del_ignore(&name)
            }
            "friend_setrank" => {
                let name = Self::pop_name(objs)?;
                let rank = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
                let body = Self::pjstr(&name)?;
                let mut packet = vec![
                    crate::proto::client::FRIEND_SETRANK,
                    body.len().wrapping_add(1) as u8,
                ];
                packet.extend(body);
                packet.push((rank as u8).wrapping_neg());
                Ok(packet)
            }
            "friend_setnotes" => {
                let note = Self::truncate_note(Self::pop_name(objs)?);
                let name = Self::pop_name(objs)?;
                let name_body = Self::pjstr(&name)?;
                let note_body = Self::pjstr(&note)?;
                let mut packet = vec![
                    crate::proto::client::FRIEND_SETNOTES,
                    name_body.len().wrapping_add(note_body.len()) as u8,
                ];
                packet.extend(name_body);
                packet.extend(note_body);
                Ok(packet)
            }
            "ignore_setnotes" => {
                let note = Self::truncate_note(Self::pop_name(objs)?);
                let name = Self::pop_name(objs)?;
                let name_body = Self::pjstr(&name)?;
                let note_body = Self::pjstr(&note)?;
                let mut packet = vec![
                    crate::proto::client::IGNORE_SETNOTES,
                    name_body.len().wrapping_add(note_body.len()) as u8,
                ];
                packet.extend(note_body);
                packet.extend(name_body);
                Ok(packet)
            }
            "clan_kickuser" => {
                // The only local guard is that a friends-chat channel is
                // joined (display name present).
                let name = Self::pop_name(objs)?;
                if self.friend_chat.display_name.is_none() {
                    return Ok(Vec::new());
                }
                Self::one_string_packet(crate::proto::client::CLAN_KICKUSER, &name, 0)
            }
            "clan_joinchat" => {
                let name = Self::pop_name(objs)?;
                if name.is_empty() {
                    return Ok(Vec::new());
                }
                Self::one_string_packet(crate::proto::client::CLAN_JOINCHAT_LEAVECHAT, &name, 0)
            }
            "clan_leavechat" => Ok(vec![crate::proto::client::CLAN_JOINCHAT_LEAVECHAT, 0]),
            "activeclanchannel_kickuser" => {
                let index =
                    usize::try_from(ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?)
                        .unwrap_or(usize::MAX);
                let Some(channel) = self.active_channel.as_ref() else {
                    return Ok(Vec::new());
                };
                let Some(user) = channel.users.get(index) else {
                    return Ok(Vec::new());
                };
                if user.rank != -1 {
                    return Ok(Vec::new());
                }
                Self::clan_channel_kick_packet(self.active_channel_affined, index, &user.name)
            }
            _ => Err(VmError::UnknownCommand {
                command: command.into(),
            }),
        }
    }
    pub(super) fn pop_name(objs: &mut Vec<String>) -> Result<String, VmError> {
        objs.pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })
    }
}
