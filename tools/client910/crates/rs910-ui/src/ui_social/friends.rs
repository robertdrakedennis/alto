//! Incoming friend and ignore list updates.

use super::{FriendEntry, FriendToast, IgnoreEntry, State};

impl State {
    pub fn apply_friend_update(&mut self, bytes: &[u8], current_world: i32) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let mut updates = Vec::new();
        while reader.pos < bytes.len() {
            let rename = reader.g1()? == 1;
            let display_name = reader.gjstr()?;
            let previous_name = reader.gjstr()?;
            let world_id = i32::from(reader.g2()?);
            let rank = i32::from(reader.g1()?);
            let flags = reader.g1()?;
            let (world_name, platform, world_flags) = if world_id > 0 {
                (reader.gjstr()?, i32::from(reader.g1()?), reader.g4s()?)
            } else {
                (String::new(), -1, 0)
            };
            let notes = reader.gjstr()?;
            updates.push((
                rename,
                FriendEntry {
                    display_name,
                    previous_name,
                    world_id,
                    world_name,
                    rank,
                    platform,
                    referrer: flags & 0x2 != 0,
                    referred: flags & 0x1 != 0,
                    notes,
                    world_flags,
                },
            ));
        }
        reader.finish("UPDATE_FRIENDLIST")?;
        for (rename, update) in updates {
            if rename {
                if let Some(existing) = self
                    .friends
                    .iter_mut()
                    .find(|friend| friend.display_name == update.previous_name)
                {
                    existing.display_name = update.display_name;
                    existing.previous_name = update.previous_name;
                } else if self.friends.len() < 400 {
                    self.friends.push(update);
                }
                continue;
            }
            if let Some(existing) = self
                .friends
                .iter_mut()
                .find(|friend| friend.display_name == update.display_name)
            {
                // A world change toggles a pending
                // login/logout toast or queues a new one.
                if existing.world_id != update.world_id {
                    let mut queue = true;
                    let world = update.world_id;
                    self.friend_toasts.retain(|toast| {
                        if toast.name == update.display_name
                            && ((world != 0 && toast.world_id == 0)
                                || (world == 0 && toast.world_id != 0))
                        {
                            queue = false;
                            false
                        } else {
                            true
                        }
                    });
                    if queue {
                        self.friend_toasts.push(FriendToast {
                            name: update.display_name.clone(),
                            world_id: world,
                            timestamp: self.now_seconds,
                        });
                    }
                }
                *existing = update;
            } else if self.friends.len() < 400 {
                self.friends.push(update);
            }
        }
        self.friends_list_state = 2;
        self.stamps.friend = true;
        // Bubble pass: swap when any one of the four
        // "should be earlier" tests holds for the right-hand friend.
        let mut remaining = self.friends.len();
        while remaining > 0 {
            let mut sorted = true;
            remaining -= 1;
            for i in 0..remaining {
                let (a, b) = (&self.friends[i], &self.friends[i + 1]);
                let swap = (current_world != a.world_id && current_world == b.world_id)
                    || (a.world_id == 0 && b.world_id != 0)
                    || (!a.referrer && b.referrer)
                    || (!a.referred && b.referred);
                if swap {
                    self.friends.swap(i, i + 1);
                    sorted = false;
                }
            }
            if sorted {
                break;
            }
        }
        Ok(())
    }
    pub fn mark_friend_list_loaded(&mut self) {
        self.friends_list_state = 1;
    }
    pub fn apply_ignore_update(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let mut updates = Vec::new();
        while reader.pos < bytes.len() {
            let flags = reader.g1()?;
            updates.push((
                flags,
                IgnoreEntry {
                    name_unfiltered: reader.gjstr()?,
                    name: reader.gjstr()?,
                    notes: reader.gjstr()?,
                    temporary: flags & 0x2 != 0,
                },
            ));
        }
        reader.finish("UPDATE_IGNORELIST")?;
        for (flags, update) in updates {
            let rename = flags & 0x1 != 0;
            if rename {
                if let Some(existing) = self
                    .ignores
                    .iter_mut()
                    .find(|ignore| ignore.name_unfiltered == update.name)
                {
                    existing.name_unfiltered = update.name_unfiltered;
                    existing.name = update.name;
                } else if self.ignores.len() < 400 {
                    self.ignores.push(update);
                }
            } else if let Some(existing) = self
                .ignores
                .iter_mut()
                .find(|ignore| ignore.name_unfiltered == update.name_unfiltered)
            {
                // Keeps the existing `temporary` flag.
                existing.name_unfiltered = update.name_unfiltered;
                existing.name = update.name;
                existing.notes = update.notes;
            } else if self.ignores.len() < 400 {
                self.ignores.push(update);
            }
        }
        self.stamps.friend = true;
        Ok(())
    }
}
