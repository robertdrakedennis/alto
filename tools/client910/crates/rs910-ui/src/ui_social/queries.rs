//! Script queries over the retained social state.

use crate::ui_vars::Value as SparseValue;

use native910::vm::{Value, VmError, VmResult};

use super::{
    from_base37, sort_names_with_slots, strip_crown_prefix, to_base37, PlayerGroupState, State,
};

impl State {
    pub fn handles(command: &str) -> bool {
        matches!(
            command,
            "friend_count"
                | "friend_getname"
                | "friend_getworld"
                | "friend_getrank"
                | "friend_getnotes"
                | "friend_getworldflags"
                | "friend_platform"
                | "friend_getworldname"
                | "friend_getslotfromname"
                | "friend_test"
                | "friend_is_referrer"
                | "friend_is_referred"
                | "ignore_count"
                | "ignore_getname"
                | "ignore_getname_unfiltered"
                | "ignore_getnotes"
                | "ignore_getslotfromname"
                | "ignore_test"
                | "ignore_is_temp"
                | "clan_getchatdisplayname"
                | "clan_getchatcount"
                | "clan_getchatusername"
                | "clan_getchatusername_unfiltered"
                | "clan_getchatuserworld"
                | "clan_getchatuserrank"
                | "clan_getchatuserworldname"
                | "clan_getchatminkick"
                | "clan_getchatrank"
                | "clan_isself"
                | "clan_getchatownername"
                | "activeclanchannel_find_affined"
                | "activeclanchannel_find_listened"
                | "activeclanchannel_getclanname"
                | "activeclanchannel_getrankkick"
                | "activeclanchannel_getranktalk"
                | "activeclanchannel_getusercount"
                | "activeclanchannel_getuserdisplayname"
                | "activeclanchannel_getuserrank"
                | "activeclanchannel_getuserworld"
                | "activeclanchannel_getuserslot"
                | "activeclanchannel_getsorteduserslot"
                | "activeclansettings_find_affined"
                | "activeclansettings_find_listened"
                | "activeclansettings_getclanname"
                | "activeclansettings_getallowunaffined"
                | "activeclansettings_getranktalk"
                | "activeclansettings_getrankkick"
                | "activeclansettings_getranklootshare"
                | "activeclansettings_getcoinshare"
                | "activeclansettings_getaffinedcount"
                | "activeclansettings_getaffineddisplayname"
                | "activeclansettings_getaffinedrank"
                | "activeclansettings_getaffinedmuted"
                | "activeclansettings_getbannedcount"
                | "activeclansettings_getbanneddisplayname"
                | "activeclansettings_getaffinedextrainfo"
                | "activeclansettings_getcurrentowner_slot"
                | "activeclansettings_getreplacementowner_slot"
                | "activeclansettings_getaffinedslot"
                | "activeclansettings_getsortedaffinedslot"
                | "activeclansettings_getaffinedjoinruneday"
                | "player_group_find"
                | "player_group_member_count"
                | "player_group_member_get_displayname"
                | "player_group_member_get_rank"
                | "player_group_member_get_status"
                | "player_group_member_get_team"
                | "player_group_member_get_last_seen_node_id"
                | "player_group_member_is_online"
                | "player_group_member_is_member"
                | "player_group_member_get_same_world_var"
                | "player_group_member_is_owner"
                | "player_group_banned_count"
                | "player_group_banned_get_displayname"
                | "player_group_get_displayname"
                | "player_group_get_max_size"
                | "player_group_get_create_mins_since_epoch"
                | "player_group_get_create_seconds_to_now"
                | "player_group_is_members_only"
                | "player_group_get_overall_status"
                | "player_group_get_owner_slot"
                | "clanprofile_find"
        )
    }
    pub(super) fn index(ints: &mut Vec<i32>) -> Result<usize, VmError> {
        usize::try_from(ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?).map_err(|_| {
            VmError::TrapFailed {
                command: "social".into(),
                reason: "negative list index".into(),
            }
        })
    }
    /// A list query past its end fails, as an out-of-range read of the original's arrays does.
    pub(super) fn out_of_range(command: &str) -> VmError {
        VmError::TrapFailed {
            command: command.into(),
            reason: "index outside the list".into(),
        }
    }
    /// The retained player group; the queries fail without one.
    pub(super) fn group_of<'a>(
        group: &'a Option<PlayerGroupState>,
        command: &str,
    ) -> Result<&'a PlayerGroupState, VmError> {
        group.as_ref().ok_or_else(|| VmError::TrapFailed {
            command: command.into(),
            reason: "player group is null".into(),
        })
    }
    /// Whether a name is a friend display name or the local player's own
    /// unfiltered name.
    pub fn friend_test(&self, name: &str) -> bool {
        self.friends
            .iter()
            .any(|friend| friend.display_name.eq_ignore_ascii_case(name))
            || name.eq_ignore_ascii_case(&self.local_player_name)
    }
    /// Whether a name matches an ignore entry (filtered or unfiltered).
    pub fn ignore_test(&self, name: &str) -> bool {
        self.ignores.iter().any(|ignore| {
            name.eq_ignore_ascii_case(&ignore.name_unfiltered)
                || name.eq_ignore_ascii_case(&ignore.name)
        })
    }
    /// Handle the friend/ignore query commands.
    /// `None` means that another host owns the command.
    pub fn dispatch(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
    ) -> VmResult<Option<Value>> {
        match command {
            "friend_count" => {
                return Ok(Some(Value::Int(match self.friends_list_state {
                    0 => -2,
                    1 => -1,
                    _ => self.friends.len() as i32,
                })))
            }
            "ignore_count" => {
                return Ok(Some(Value::Int(if self.friends_list_state == 0 {
                    -1
                } else {
                    self.ignores.len() as i32
                })))
            }
            "friend_getname" => {
                // Only a ready list (state 2) yields a name.
                let index = Self::index(ints)?;
                let friend = self
                    .friends
                    .get(index)
                    .filter(|_| self.friends_list_state == 2);
                objs.extend([
                    friend.map_or_else(String::new, |entry| entry.display_name.clone()),
                    friend.map_or_else(String::new, |entry| entry.previous_name.clone()),
                ]);
                return Ok(None);
            }
            "friend_getworld"
            | "friend_getrank"
            | "friend_getworldflags"
            | "friend_platform"
            | "friend_is_referrer"
            | "friend_is_referred"
            | "friend_getworldname"
            | "friend_getnotes" => {
                // Only a ready list (state 2) answers; friend_platform also answers 0 for a negative index.
                let raw = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
                let index = match usize::try_from(raw) {
                    Ok(index) => index,
                    Err(_) if command == "friend_platform" => usize::MAX,
                    Err(_) => {
                        return Err(VmError::TrapFailed {
                            command: command.into(),
                            reason: "negative friend index".into(),
                        })
                    }
                };
                let friend = self
                    .friends
                    .get(index)
                    .filter(|_| self.friends_list_state == 2);
                match command {
                    "friend_getworld" => ints.push(friend.map_or(0, |entry| entry.world_id)),
                    "friend_getrank" => ints.push(friend.map_or(0, |entry| entry.rank)),
                    "friend_getworldflags" => {
                        ints.push(friend.map_or(0, |entry| entry.world_flags))
                    }
                    "friend_platform" => ints.push(friend.map_or(0, |entry| entry.platform)),
                    "friend_is_referrer" => {
                        ints.push(i32::from(friend.is_some_and(|entry| entry.referrer)))
                    }
                    "friend_is_referred" => {
                        ints.push(i32::from(friend.is_some_and(|entry| entry.referred)))
                    }
                    "friend_getworldname" => {
                        objs.push(friend.map_or_else(String::new, |entry| entry.world_name.clone()))
                    }
                    "friend_getnotes" => {
                        objs.push(friend.map_or_else(String::new, |entry| entry.notes.clone()))
                    }
                    _ => unreachable!(),
                }
                return Ok(None);
            }
            "ignore_getname" => {
                // Any loaded state (non-zero) answers.
                let index = Self::index(ints)?;
                let ignore = self
                    .ignores
                    .get(index)
                    .filter(|_| self.friends_list_state != 0);
                objs.extend([
                    ignore.map_or_else(String::new, |entry| entry.name_unfiltered.clone()),
                    ignore.map_or_else(String::new, |entry| entry.name.clone()),
                ]);
                return Ok(None);
            }
            "ignore_getname_unfiltered" => {
                let index = Self::index(ints)?;
                objs.push(
                    self.ignores
                        .get(index)
                        .filter(|_| self.friends_list_state != 0)
                        .map_or_else(String::new, |entry| entry.name_unfiltered.clone()),
                );
                return Ok(None);
            }
            "ignore_getnotes" => {
                let index = Self::index(ints)?;
                objs.push(
                    self.ignores
                        .get(index)
                        .filter(|_| self.friends_list_state != 0)
                        .map_or_else(String::new, |entry| entry.notes.clone()),
                );
                return Ok(None);
            }
            "ignore_is_temp" => {
                let index = Self::index(ints)?;
                let entry = self
                    .ignores
                    .get(index)
                    .ok_or_else(|| Self::out_of_range(command))?;
                ints.push(i32::from(entry.temporary));
                return Ok(None);
            }
            "clan_getchatdisplayname" | "clan_getchatownername" => {
                objs.push(if command == "clan_getchatdisplayname" {
                    // Round-trips through base 37, empty when it does not decode.
                    self.friend_chat
                        .display_name
                        .as_deref()
                        .map(|name| from_base37(to_base37(name)).unwrap_or_default())
                        .unwrap_or_default()
                } else {
                    self.friend_chat.owner_name.clone().unwrap_or_default()
                });
                return Ok(None);
            }
            "player_group_find" => {
                ints.push(i32::from(self.player_group_present));
                return Ok(None);
            }
            "player_group_member_count" => {
                ints.push(Self::group_of(&self.player_group, command)?.members.len() as i32);
                return Ok(None);
            }
            "player_group_member_get_displayname"
            | "player_group_member_get_rank"
            | "player_group_member_get_status"
            | "player_group_member_get_team"
            | "player_group_member_get_last_seen_node_id"
            | "player_group_member_is_online"
            | "player_group_member_is_member"
            | "player_group_member_is_owner" => {
                let index = Self::index(ints)?;
                let group = Self::group_of(&self.player_group, command)?;
                // The owner query compares slots and never reads a member.
                if command == "player_group_member_is_owner" {
                    ints.push(i32::from(group.owner_slot == index as i32));
                    return Ok(None);
                }
                let member = group
                    .members
                    .get(index)
                    .ok_or_else(|| Self::out_of_range(command))?;
                match command {
                    "player_group_member_get_displayname" => objs.push(member.display_name.clone()),
                    "player_group_member_get_rank" => ints.push(member.rank),
                    "player_group_member_get_status" => ints.push(member.status),
                    "player_group_member_get_team" => ints.push(member.team),
                    "player_group_member_get_last_seen_node_id" => ints.push(member.node_id),
                    "player_group_member_is_online" => ints.push(i32::from(member.online)),
                    "player_group_member_is_member" => ints.push(i32::from(member.members)),
                    _ => unreachable!(),
                }
                return Ok(None);
            }
            "player_group_member_get_same_world_var" => {
                // Varp branch; the varbit branch
                // is resolved by the Engine host, which owns varbit types.
                if ints.len() < 3 {
                    return Err(VmError::StackUnderflow { stack: "int" });
                }
                let args = ints.split_off(ints.len() - 3);
                let member = self
                    .player_group
                    .as_ref()
                    .and_then(|group| group.members.get(usize::try_from(args[0]).ok()?))
                    .ok_or_else(|| VmError::TrapFailed {
                        command: command.into(),
                        reason: "player group member missing".into(),
                    })?;
                let value = member.variables.as_ref().and_then(|v| v.get(&args[2]));
                return Ok(Some(match (value, self.group_var_types.get(&args[2])) {
                    (Some(SparseValue::Int(value)), _) => Value::Int(*value),
                    (Some(SparseValue::Long(value)), _) => Value::Long(*value),
                    (Some(SparseValue::String(value)), _) => {
                        Value::Str(String::from_utf16_lossy(value))
                    }
                    (_, Some(1)) => Value::Long(0),
                    (_, Some(2)) => Value::Str(String::new()),
                    _ => Value::Int(0),
                }));
            }
            "player_group_banned_count" => {
                ints.push(Self::group_of(&self.player_group, command)?.banned.len() as i32);
                return Ok(None);
            }
            "player_group_banned_get_displayname" => {
                let index = Self::index(ints)?;
                let group = Self::group_of(&self.player_group, command)?;
                objs.push(
                    group
                        .banned
                        .get(index)
                        .ok_or_else(|| Self::out_of_range(command))?
                        .clone(),
                );
                return Ok(None);
            }
            "player_group_get_displayname" => {
                objs.push(
                    Self::group_of(&self.player_group, command)?
                        .display_name
                        .clone(),
                );
                return Ok(None);
            }
            "player_group_get_max_size" => {
                ints.push(Self::group_of(&self.player_group, command)?.max_size);
                return Ok(None);
            }
            "player_group_get_create_mins_since_epoch" => {
                let group = Self::group_of(&self.player_group, command)?;
                ints.push((group.creation_time / 60_000) as i32);
                return Ok(None);
            }
            "player_group_get_create_seconds_to_now" => {
                let group = Self::group_of(&self.player_group, command)?;
                let now = crate::logic_clock::monotonic_millis();
                ints.push(now.wrapping_sub(group.creation_time) as i32 / 1000);
                return Ok(None);
            }
            "player_group_is_members_only" => {
                ints.push(i32::from(
                    Self::group_of(&self.player_group, command)?.members_only,
                ));
                return Ok(None);
            }
            "player_group_get_overall_status" => {
                let group = Self::group_of(&self.player_group, command)?;
                let status = if group.members.is_empty() {
                    0
                } else {
                    match group.members[0].status {
                        3 | 2 => group.members[0].status,
                        _ if group.members.iter().any(|member| member.status == 0) => 0,
                        _ => 1,
                    }
                };
                ints.push(status);
                return Ok(None);
            }
            "player_group_get_owner_slot" => {
                ints.push(Self::group_of(&self.player_group, command)?.owner_slot);
                return Ok(None);
            }
            "clanprofile_find" => {
                ints.push(i32::from(self.clan_vars.is_some()));
                return Ok(None);
            }
            "activeclansettings_find_affined" | "activeclansettings_find_listened" => {
                let affined = command == "activeclansettings_find_affined";
                let settings = if affined {
                    self.affined_settings.clone()
                } else {
                    self.listened_settings.clone()
                };
                // A miss keeps the previous
                // active settings.
                ints.push(i32::from(settings.is_some()));
                if settings.is_some() {
                    self.active_settings_affined = affined;
                    self.active_settings = settings;
                    self.publish_active_settings();
                }
                return Ok(None);
            }
            "activeclansettings_getclanname"
            | "activeclansettings_getallowunaffined"
            | "activeclansettings_getranktalk"
            | "activeclansettings_getrankkick"
            | "activeclansettings_getranklootshare"
            | "activeclansettings_getcoinshare"
            | "activeclansettings_getaffinedcount"
            | "activeclansettings_getbannedcount"
            | "activeclansettings_getcurrentowner_slot"
            | "activeclansettings_getreplacementowner_slot" => {
                let settings =
                    self.active_settings
                        .as_ref()
                        .ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "active clan settings is null".into(),
                        })?;
                match command {
                    "activeclansettings_getclanname" => objs.push(settings.clan_name.clone()),
                    "activeclansettings_getallowunaffined" => {
                        ints.push(i32::from(settings.allow_unaffined))
                    }
                    "activeclansettings_getranktalk" => ints.push(settings.rank_talk),
                    "activeclansettings_getrankkick" => ints.push(settings.rank_kick),
                    "activeclansettings_getranklootshare" => ints.push(settings.rank_lootshare),
                    "activeclansettings_getcoinshare" => ints.push(settings.coinshare),
                    "activeclansettings_getaffinedcount" => {
                        ints.push(settings.members.len() as i32)
                    }
                    "activeclansettings_getbannedcount" => ints.push(settings.banned.len() as i32),
                    "activeclansettings_getcurrentowner_slot" => {
                        ints.push(settings.current_owner_slot)
                    }
                    "activeclansettings_getreplacementowner_slot" => {
                        ints.push(settings.replacement_owner_slot)
                    }
                    _ => unreachable!(),
                }
                return Ok(None);
            }
            "activeclansettings_getaffineddisplayname"
            | "activeclansettings_getaffinedrank"
            | "activeclansettings_getaffinedmuted"
            | "activeclansettings_getaffinedjoinruneday"
            | "activeclansettings_getbanneddisplayname" => {
                let index = Self::index(ints)?;
                let settings =
                    self.active_settings
                        .as_ref()
                        .ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "active clan settings is null".into(),
                        })?;
                if command == "activeclansettings_getbanneddisplayname" {
                    objs.push(
                        settings
                            .banned
                            .get(index)
                            .ok_or_else(|| Self::out_of_range(command))?
                            .clone(),
                    );
                } else {
                    let member = settings
                        .members
                        .get(index)
                        .ok_or_else(|| Self::out_of_range(command))?;
                    match command {
                        "activeclansettings_getaffineddisplayname" => {
                            objs.push(member.display_name.clone())
                        }
                        "activeclansettings_getaffinedrank" => ints.push(member.rank),
                        "activeclansettings_getaffinedmuted" => ints.push(i32::from(member.muted)),
                        "activeclansettings_getaffinedjoinruneday" => {
                            ints.push(member.joined_runedays)
                        }
                        _ => unreachable!(),
                    }
                }
                return Ok(None);
            }
            "activeclansettings_getaffinedextrainfo" => {
                let end = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
                let start = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
                let index = Self::index(ints)?;
                let settings =
                    self.active_settings
                        .as_ref()
                        .ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "active clan settings is null".into(),
                        })?;
                let member = settings
                    .members
                    .get(index)
                    .ok_or_else(|| Self::out_of_range(command))?
                    .extra;
                // Extracts the bit range `start..=end` of the member's extra info.
                let mask = if end == 31 {
                    -1
                } else {
                    (1_i32.wrapping_shl((end + 1) as u32)).wrapping_sub(1)
                };
                ints.push(((member & mask) as u32).wrapping_shr(start as u32) as i32);
                return Ok(None);
            }
            "activeclansettings_getaffinedslot" => {
                let name = Self::pop_name(objs)?;
                let settings =
                    self.active_settings
                        .as_ref()
                        .ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "active clan settings is null".into(),
                        })?;
                // Slot of the member with this display name, or -1.
                ints.push(if name.is_empty() {
                    -1
                } else {
                    settings
                        .members
                        .iter()
                        .position(|m| m.display_name == name)
                        .map_or(-1, |i| i as i32)
                });
                return Ok(None);
            }
            "activeclansettings_getsortedaffinedslot" => {
                let ordinal = Self::index(ints)?;
                let settings =
                    self.active_settings
                        .as_ref()
                        .ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "active clan settings is null".into(),
                        })?;
                // Slot at the given position of the lowercase-name ordering.
                let mut names: Vec<Option<Vec<u16>>> = settings
                    .members
                    .iter()
                    .map(|m| Some(m.display_name.to_lowercase().encode_utf16().collect()))
                    .collect();
                let mut slots: Vec<i32> = (0..names.len() as i32).collect();
                sort_names_with_slots(&mut names, &mut slots);
                ints.push(*slots.get(ordinal).ok_or_else(|| VmError::TrapFailed {
                    command: command.into(),
                    reason: "sorted affined slot out of range".into(),
                })?);
                return Ok(None);
            }
            "clan_getchatcount" => {
                return Ok(Some(Value::Int(
                    if self.friend_chat.display_name.is_some() {
                        self.friend_chat.users.len() as i32
                    } else {
                        0
                    },
                )))
            }
            "clan_getchatminkick" | "clan_getchatrank" => {
                ints.push(if command == "clan_getchatminkick" {
                    self.friend_chat.min_kick
                } else {
                    self.friend_chat.rank
                });
                return Ok(None);
            }
            "clan_getchatusername"
            | "clan_getchatusername_unfiltered"
            | "clan_getchatuserworld"
            | "clan_getchatuserrank"
            | "clan_getchatuserworldname"
            | "clan_isself" => {
                let index = Self::index(ints)?;
                let user = self.friend_chat.users.get(index);
                match command {
                    "clan_getchatusername" => {
                        objs.push(user.map_or_else(String::new, |u| u.name.clone()))
                    }
                    "clan_getchatusername_unfiltered" => {
                        objs.push(user.map_or_else(String::new, |u| u.name_unfiltered.clone()))
                    }
                    "clan_getchatuserworld" => ints.push(user.map_or(0, |u| u.world)),
                    "clan_getchatuserrank" => ints.push(user.map_or(0, |u| u.rank)),
                    "clan_getchatuserworldname" => {
                        objs.push(user.map_or_else(String::new, |u| u.world_name.clone()))
                    }
                    "clan_isself" => ints.push(i32::from(user.is_some_and(|u| {
                        u.name_unfiltered
                            .eq_ignore_ascii_case(&self.local_player_name)
                    }))),
                    _ => unreachable!(),
                }
                return Ok(None);
            }
            "activeclanchannel_find_affined" | "activeclanchannel_find_listened" => {
                let channel = if command == "activeclanchannel_find_affined" {
                    self.affined_channel.clone()
                } else {
                    self.listened_channel.clone()
                };
                // A miss keeps the previous
                // active channel.
                ints.push(i32::from(channel.is_some()));
                if channel.is_some() {
                    self.active_channel_affined = command == "activeclanchannel_find_affined";
                    self.active_channel_generation = if self.active_channel_affined {
                        self.affined_channel_generation
                    } else {
                        self.listened_channel_generation
                    };
                    self.active_channel = channel;
                }
                return Ok(None);
            }
            "activeclanchannel_getclanname"
            | "activeclanchannel_getrankkick"
            | "activeclanchannel_getranktalk"
            | "activeclanchannel_getusercount"
            | "activeclanchannel_getuserdisplayname"
            | "activeclanchannel_getuserrank"
            | "activeclanchannel_getuserworld"
            | "activeclanchannel_getuserslot"
            | "activeclanchannel_getsorteduserslot" => {
                let channel = self
                    .active_channel
                    .as_ref()
                    .ok_or_else(|| VmError::TrapFailed {
                        command: command.into(),
                        reason: "active clan channel is null".into(),
                    })?;
                match command {
                    "activeclanchannel_getclanname" => objs.push(channel.clan_name.clone()),
                    "activeclanchannel_getrankkick" => ints.push(channel.rank_kick),
                    "activeclanchannel_getranktalk" => ints.push(channel.rank_talk),
                    "activeclanchannel_getusercount" => ints.push(channel.users.len() as i32),
                    "activeclanchannel_getuserdisplayname" => {
                        let index = Self::index(ints)?;
                        objs.push(
                            channel
                                .users
                                .get(index)
                                .ok_or_else(|| Self::out_of_range(command))?
                                .name
                                .clone(),
                        );
                    }
                    "activeclanchannel_getuserrank" => {
                        let index = Self::index(ints)?;
                        ints.push(
                            channel
                                .users
                                .get(index)
                                .ok_or_else(|| Self::out_of_range(command))?
                                .rank,
                        );
                    }
                    "activeclanchannel_getuserworld" => {
                        let index = Self::index(ints)?;
                        ints.push(
                            channel
                                .users
                                .get(index)
                                .ok_or_else(|| Self::out_of_range(command))?
                                .world,
                        );
                    }
                    "activeclanchannel_getuserslot" => {
                        let name = Self::pop_name(objs)?;
                        ints.push(
                            channel
                                .users
                                .iter()
                                .position(|user| user.name.eq_ignore_ascii_case(&name))
                                .map_or(-1, |index| index as i32),
                        );
                    }
                    "activeclanchannel_getsorteduserslot" => {
                        // Slot at the given position of the name ordering.
                        let ordinal = Self::index(ints)?;
                        let mut names: Vec<Option<Vec<u16>>> = channel
                            .users
                            .iter()
                            .map(|u| Some(u.name.encode_utf16().collect()))
                            .collect();
                        let mut slots: Vec<i32> = (0..names.len() as i32).collect();
                        sort_names_with_slots(&mut names, &mut slots);
                        ints.push(*slots.get(ordinal).ok_or_else(|| VmError::TrapFailed {
                            command: command.into(),
                            reason: "sorted user slot out of range".into(),
                        })?);
                    }
                    _ => unreachable!(),
                }
                return Ok(None);
            }
            "friend_getslotfromname" | "friend_test" => {
                let name = strip_crown_prefix(Self::pop_name(objs)?);
                ints.push(if command == "friend_test" {
                    i32::from(self.friend_test(&name))
                } else {
                    // Slot of the friend with this display name, or -1.
                    self.friends
                        .iter()
                        .position(|friend| name.eq_ignore_ascii_case(&friend.display_name))
                        .map_or(-1, |index| index as i32)
                });
                return Ok(None);
            }
            "ignore_getslotfromname" | "ignore_test" => {
                let name = strip_crown_prefix(Self::pop_name(objs)?);
                ints.push(if command == "ignore_test" {
                    i32::from(self.ignore_test(&name))
                } else if name.is_empty() {
                    // An empty name has no slot.
                    -1
                } else {
                    self.ignores
                        .iter()
                        .position(|ignore| name.eq_ignore_ascii_case(&ignore.name_unfiltered))
                        .map_or(-1, |index| index as i32)
                });
                return Ok(None);
            }
            _ => {}
        }
        Ok(None)
    }
}
