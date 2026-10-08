//! Friends-chat and clan-channel roster updates.

use super::{namespace_normalize, ClanChannel, ClanChannelUser, FriendChatUser, State};

impl State {
    /// The namespace-normalized unfiltered name that orders the friends-chat
    /// roster.
    pub(super) fn friend_chat_key(user: &FriendChatUser) -> String {
        namespace_normalize(&user.name_unfiltered).unwrap_or_default()
    }
    /// Applies `UPDATE_FRIENDCHAT_CHANNEL_FULL`.
    pub fn apply_friend_chat_full(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.stamps.clan = true;
        if bytes.is_empty() {
            // An empty payload clears names, count and users; rank and min-kick stay.
            self.friend_chat.owner_name = None;
            self.friend_chat.display_name = None;
            self.friend_chat.users.clear();
            return Ok(());
        }
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let owner_name = reader.gjstr()?;
        if reader.g1()? == 1 {
            let _ = reader.gjstr()?;
        }
        let display_name = reader.gjstr()?;
        let min_kick = i32::from(reader.g1()? as i8);
        let count = reader.g1()?;
        self.friend_chat.owner_name = Some(owner_name);
        self.friend_chat.display_name = Some(display_name);
        self.friend_chat.min_kick = min_kick;
        if count == 255 {
            // :10369-10372 keeps the previous roster.
            return Ok(());
        }
        let mut users = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            let name = reader.gjstr()?;
            let name_unfiltered = if reader.g1()? == 1 {
                reader.gjstr()?
            } else {
                name.clone()
            };
            let user = FriendChatUser {
                name,
                name_unfiltered,
                world: i32::from(reader.g2()?),
                rank: i32::from(reader.g1()? as i8),
                world_name: reader.gjstr()?,
            };
            if user.name_unfiltered == self.local_player_name {
                self.friend_chat.rank = user.rank;
            }
            users.push(user);
        }
        reader.finish("UPDATE_FRIENDCHAT_CHANNEL_FULL")?;
        // :10393-10409 bubble sort on the normalized names.
        let mut remaining = users.len();
        while remaining > 0 {
            let mut sorted = true;
            remaining -= 1;
            for i in 0..remaining {
                if Self::friend_chat_key(&users[i]) > Self::friend_chat_key(&users[i + 1]) {
                    users.swap(i, i + 1);
                    sorted = false;
                }
            }
            if sorted {
                break;
            }
        }
        self.friend_chat.users = users;
        Ok(())
    }
    /// Applies `UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER`.
    pub fn apply_friend_chat_single(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let name = reader.gjstr()?;
        let name_unfiltered = if reader.g1()? == 1 {
            reader.gjstr()?
        } else {
            name.clone()
        };
        let world = i32::from(reader.g2()?);
        let rank = i32::from(reader.g1()? as i8);
        if rank == -128 {
            reader.finish("UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER")?;
            if self.friend_chat.users.is_empty() {
                return Ok(());
            }
            if let Some(index) = self
                .friend_chat
                .users
                .iter()
                .position(|user| user.name_unfiltered == name_unfiltered && user.world == world)
            {
                self.friend_chat.users.remove(index);
            }
            self.stamps.clan = true;
            return Ok(());
        }
        let world_name = reader.gjstr()?;
        reader.finish("UPDATE_FRIENDCHAT_CHANNEL_SINGLEUSER")?;
        let user = FriendChatUser {
            name,
            name_unfiltered,
            world,
            rank,
            world_name,
        };
        let key = Self::friend_chat_key(&user);
        let mut index = self.friend_chat.users.len() as isize - 1;
        while index >= 0 {
            let existing = &mut self.friend_chat.users[index as usize];
            let order = Self::friend_chat_key(existing).cmp(&key);
            if order == std::cmp::Ordering::Equal {
                existing.world = world;
                existing.rank = rank;
                existing.world_name = user.world_name;
                if user.name_unfiltered == self.local_player_name {
                    self.friend_chat.rank = rank;
                }
                self.stamps.clan = true;
                return Ok(());
            }
            if order == std::cmp::Ordering::Less {
                break;
            }
            index -= 1;
        }
        if self.friend_chat.users.len() >= 100 {
            return Ok(());
        }
        if user.name_unfiltered == self.local_player_name {
            self.friend_chat.rank = rank;
        }
        self.friend_chat.users.insert((index + 1) as usize, user);
        self.stamps.clan = true;
        Ok(())
    }
    pub fn apply_clan_channel_full(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let affined = reader.g1()? == 1;
        let channel = if reader.remaining() == 0 {
            None
        } else {
            let flags = reader.g1()?;
            let use_user_hashes = flags & 0x1 != 0;
            // `use_display_names` starts true; the 0x2 flag can only set it
            // again.
            let use_display_names = true;
            let version = if flags & 0x4 != 0 { reader.g1()? } else { 2 };
            let node_id = reader.g8()?;
            let update_num = reader.g8()?;
            let clan_name = reader.gjstr()?;
            let _ = reader.g1()?;
            let rank_kick = i32::from(reader.g1()? as i8);
            let rank_talk = i32::from(reader.g1()? as i8);
            let count = usize::from(reader.g2()?);
            let mut users = Vec::with_capacity(count);
            for _ in 0..count {
                if use_user_hashes {
                    let _ = reader.g8()?;
                }
                let name = if use_display_names {
                    reader.gjstr()?
                } else {
                    String::new()
                };
                let rank = i32::from(reader.g1()? as i8);
                let world = i32::from(reader.g2()?);
                if version >= 3 {
                    let _ = reader.g1()?;
                }
                users.push(ClanChannelUser { name, rank, world });
            }
            Some(ClanChannel {
                use_user_hashes,
                use_display_names,
                node_id,
                update_num,
                clan_name,
                rank_talk,
                rank_kick,
                users,
            })
        };
        reader.finish("CLANCHANNEL_FULL")?;
        // Stamps the clan-channel transmit redraw cycle. A new
        // object replaces the retained one; an earlier active reference keeps
        // pointing at the old object (new generation).
        self.stamps.clan_channel = true;
        if affined {
            self.affined_channel = channel;
            self.affined_channel_generation += 1;
        } else {
            self.listened_channel = channel;
            self.listened_channel_generation += 1;
        }
        Ok(())
    }
    pub fn apply_clan_channel_delta(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let affined = reader.g1()? == 1;
        let expected = if affined {
            self.affined_channel.as_mut()
        } else {
            self.listened_channel.as_mut()
        };
        let channel =
            expected.ok_or_else(|| anyhow::anyhow!("CLANCHANNEL_DELTA without channel"))?;
        let clan_hash = reader.g8()?;
        let update_num = reader.g8()?;
        anyhow::ensure!(
            channel.node_id == clan_hash && channel.update_num == update_num,
            "CLANCHANNEL_DELTA sequence mismatch"
        );
        loop {
            let entry = reader.g1()?;
            match entry {
                0 => break,
                1 => {
                    // ClanRosterDelta.AddUser.decode: `g1() != 255` rewinds
                    // before the 8-byte hash.
                    let _ = Self::optional_hash(&mut reader)?;
                    let name = reader.fastgstr()?.unwrap_or_default();
                    let world = i32::from(reader.g2()?);
                    let rank = i32::from(reader.g1()? as i8);
                    let _ = reader.g8()?;
                    channel.users.push(ClanChannelUser { name, rank, world });
                }
                2 | 5 => {
                    let v2 = entry == 5;
                    if v2 {
                        let _ = reader.g1()?;
                    }
                    let index = usize::from(reader.g2()?);
                    let rank = i32::from(reader.g1()? as i8);
                    let world = i32::from(reader.g2()?);
                    let _ = reader.g8()?;
                    let name = reader.gjstr()?;
                    if let Some(user) = channel.users.get_mut(index) {
                        user.rank = rank;
                        user.world = world;
                        user.name = name;
                    }
                    if v2 {
                        let _ = reader.g1()?;
                    }
                }
                3 => {
                    let index = usize::from(reader.g2()?);
                    let _ = reader.g1()?;
                    let _ = Self::optional_hash(&mut reader)?;
                    if index < channel.users.len() {
                        channel.users.remove(index);
                    }
                }
                4 => {
                    channel.clan_name = reader.fastgstr()?.unwrap_or_default();
                    if !channel.clan_name.is_empty() {
                        let _ = reader.g1()?;
                        channel.rank_talk = i32::from(reader.g1()? as i8);
                        channel.rank_kick = i32::from(reader.g1()? as i8);
                    }
                }
                opcode => anyhow::bail!("CLANCHANNEL_DELTA unknown entry {opcode}"),
            }
        }
        reader.finish("CLANCHANNEL_DELTA")?;
        channel.update_num = channel.update_num.wrapping_add(1);
        // Stamps the clan-channel transmit; the active reference is the same
        // object while its generation holds.
        let generation = if affined {
            self.affined_channel_generation
        } else {
            self.listened_channel_generation
        };
        let refreshed = channel.clone();
        if self.active_channel.is_some()
            && self.active_channel_affined == affined
            && self.active_channel_generation == generation
        {
            self.active_channel = Some(refreshed);
        }
        self.stamps.clan_channel = true;
        Ok(())
    }
}
