//! Retained friend/ignore list state from the normal packet path.
//!
//! The server owns the lists. This module only decodes the incoming list
//! updates and exposes the CS2 queries which read that retained state.

mod names;
use names::friend_related;
use names::strip_crown_prefix;
pub use names::{
    from_base37, namespace_normalize, sort_names_with_slots, to_base37, trim_control_chars,
    SocialText,
};
mod quick_chat;
pub use quick_chat::QuickChatStore;
mod channels;
mod clan_settings;
mod friends;
mod mutations;
mod player_groups;
mod queries;

use crate::ui_vars::Value as SparseValue;

use std::collections::BTreeMap;

/// A queued friends-list login/logout toast.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FriendToast {
    pub name: String,
    pub world_id: i32,
    /// Monotonic seconds at creation.
    pub timestamp: i64,
}

/// Social owners that stamp the interface transmit-redraw cycle.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TransmitStamps {
    pub friend: bool,
    pub clan: bool,
    pub clan_settings: bool,
    pub clan_channel: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendEntry {
    pub display_name: String,
    pub previous_name: String,
    pub world_id: i32,
    pub world_name: String,
    pub rank: i32,
    pub platform: i32,
    pub referrer: bool,
    pub referred: bool,
    pub notes: String,
    pub world_flags: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IgnoreEntry {
    pub name_unfiltered: String,
    pub name: String,
    pub notes: String,
    pub temporary: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FriendChatUser {
    pub name: String,
    pub name_unfiltered: String,
    pub world: i32,
    pub rank: i32,
    pub world_name: String,
}

#[derive(Clone, Debug, Default)]
pub struct FriendChat {
    pub owner_name: Option<String>,
    pub display_name: Option<String>,
    pub min_kick: i32,
    pub rank: i32,
    pub users: Vec<FriendChatUser>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanChannelUser {
    pub name: String,
    pub rank: i32,
    pub world: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanChannel {
    pub use_user_hashes: bool,
    pub use_display_names: bool,
    pub node_id: u64,
    pub update_num: u64,
    pub clan_name: String,
    pub rank_talk: i32,
    pub rank_kick: i32,
    pub users: Vec<ClanChannelUser>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanSettingsMember {
    pub hash: i64,
    pub display_name: String,
    pub rank: i32,
    pub extra: i32,
    pub joined_runedays: i32,
    pub muted: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClanSettingValue {
    Int(i32),
    Long(i64),
    String(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClanSettingsState {
    pub use_user_hashes: bool,
    pub use_display_names: bool,
    pub owner: i64,
    pub update_num: i32,
    pub clan_name: String,
    pub allow_unaffined: bool,
    pub rank_talk: i32,
    pub rank_kick: i32,
    pub rank_lootshare: i32,
    pub coinshare: i32,
    pub members: Vec<ClanSettingsMember>,
    pub banned: Vec<String>,
    pub settings: BTreeMap<i32, ClanSettingValue>,
    pub current_owner_slot: i32,
    pub replacement_owner_slot: i32,
}

/// The settings map of the script state's active clan settings; `None` while no active clan settings are installed.
/// `Arc<Mutex>` (not `Rc`) because the owning `ui_vars::State` is built on
/// the loading worker thread (`CacheConfigs`/`Game` decode is `Send`).
pub type ClanSettingsDomain =
    std::sync::Arc<std::sync::Mutex<Option<BTreeMap<i32, ClanSettingValue>>>>;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerGroupMemberState {
    pub group_uid: i64,
    pub display_name: String,
    pub members: bool,
    pub online: bool,
    pub stats: Vec<i32>,
    pub vars: BTreeMap<i32, SparseValue>,
    /// Per-member variable container, written only by PLAYER_GROUP_VARPS and
    /// read by `player_group_member_get_same_world_var`.
    pub variables: Option<BTreeMap<i32, SparseValue>>,
    pub node_id: i32,
    pub rank: i32,
    pub status: i32,
    pub team: i32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlayerGroupState {
    pub hashcode: i64,
    pub update_num: i32,
    pub field: i32,
    pub creation_time: i64,
    pub display_name: String,
    pub members_only: bool,
    pub has_uid: bool,
    pub has_display_name: bool,
    pub max_size: i32,
    pub members: Vec<PlayerGroupMemberState>,
    pub banned: Vec<String>,
    pub vars: BTreeMap<i32, SparseValue>,
    pub owner_slot: i32,
}

impl PlayerGroupState {
    /// The owner is the first member with the highest rank.
    fn recompute_owner(&mut self) {
        let mut best: Option<(usize, i32)> = None;
        for (index, member) in self.members.iter().enumerate() {
            if best.is_none_or(|(_, rank)| member.rank > rank) {
                best = Some((index, member.rank));
            }
        }
        self.owner_slot = best.map_or(-1, |(index, _)| index as i32);
    }
}

impl ClanSettingsState {
    fn recompute_owner(&mut self) {
        self.current_owner_slot = -1;
        self.replacement_owner_slot = -1;
        let mut best_rank = i32::MIN;
        for (index, member) in self.members.iter().enumerate() {
            if member.rank > best_rank {
                if best_rank == 125 {
                    self.replacement_owner_slot = self.current_owner_slot;
                }
                best_rank = member.rank;
                self.current_owner_slot = index as i32;
            } else if self.replacement_owner_slot == -1 && member.rank == 125 {
                self.replacement_owner_slot = index as i32;
            }
        }
        if self.current_owner_slot >= 0 {
            self.members[self.current_owner_slot as usize].rank = 126;
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct State {
    /// Friends list state: 0 loading, 1 loaded marker, 2 ready.
    pub friends_list_state: i32,
    pub friends: Vec<FriendEntry>,
    pub ignores: Vec<IgnoreEntry>,
    pub local_player_name: String,
    /// Title of the local player, shown with the name.
    pub local_player_title: Option<String>,
    pub friend_chat: FriendChat,
    pub affined_channel: Option<ClanChannel>,
    pub listened_channel: Option<ClanChannel>,
    pub active_channel: Option<ClanChannel>,
    /// Object identity for `affinedClanChannel`/`listenedClanChannel`: each
    /// CLANCHANNEL_FULL installs a new object.
    pub affined_channel_generation: u64,
    pub listened_channel_generation: u64,
    pub active_channel_generation: u64,
    pub active_channel_affined: bool,
    pub affined_settings: Option<ClanSettingsState>,
    pub listened_settings: Option<ClanSettingsState>,
    pub active_settings: Option<ClanSettingsState>,
    pub active_settings_affined: bool,
    /// The clan-setting variable domain as installed on the script state for
    /// the active clan settings. Shared with the variable owner
    /// (`ui_vars::State::clan_settings`), which reads it read-only.
    pub active_settings_domain: ClanSettingsDomain,
    /// Current player group presence/display owner. Member and
    /// group-var domains are retained by the normal full/delta packet path.
    pub player_group_present: bool,
    pub player_group_name: String,
    pub player_group: Option<PlayerGroupState>,
    /// Sparse clan profile variables. The map is installed by
    /// VARCLAN_ENABLE, cleared by VARCLAN_DISABLE, and lazily created by a
    /// value packet.
    pub clan_vars: Option<BTreeMap<i32, SparseValue>>,
    /// Whether the local player is a members account, synced by the retained
    /// runtime; it selects the friend/ignore list limits.
    pub player_is_members: bool,
    /// System chat rows (type, text) produced by social
    /// mutations, drained into the retained chat history by the caller.
    pub system_messages: Vec<(i32, String)>,
    /// The friends-list toast queue.
    pub friend_toasts: Vec<FriendToast>,
    /// Monotonic seconds supplied by the packet owner for new toasts.
    pub now_seconds: i64,
    /// Pending transmit-redraw-cycle stamps.
    pub stamps: TransmitStamps,
    /// Base types of player varps seen through PLAYER_GROUP_VARPS.
    pub group_var_types: BTreeMap<i32, u8>,
}

impl State {
    /// Take the pending transmit stamps for the runtime redraw-cycle owner.
    pub fn take_stamps(&mut self) -> TransmitStamps {
        std::mem::take(&mut self.stamps)
    }
    /// The toast-queue walk: every
    /// entry older than five seconds becomes a type-5 chat line.
    pub fn poll_friend_toasts(&mut self, now_seconds: i64) -> Vec<String> {
        let mut out = Vec::new();
        self.friend_toasts.retain(|toast| {
            if toast.timestamp < now_seconds - 5 {
                if toast.world_id > 0 {
                    out.push(format!("{}{}", toast.name, SocialText::FriendLogin.text()));
                }
                if toast.world_id == 0 {
                    out.push(format!("{}{}", toast.name, SocialText::FriendLogout.text()));
                }
                false
            } else {
                true
            }
        });
        out
    }
}

#[cfg(test)]
mod tests;
