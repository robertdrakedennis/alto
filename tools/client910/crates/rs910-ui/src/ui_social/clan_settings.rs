//! Clan-settings full and delta decoding and active variable publication.

use super::{ClanSettingValue, ClanSettingsMember, ClanSettingsState, State};

impl State {
    pub(super) fn optional_hash(
        reader: &mut crate::server_prot::PayloadReader<'_>,
    ) -> anyhow::Result<i64> {
        if reader.g1()? == 255 {
            Ok(-1)
        } else {
            reader.pos = reader.pos.saturating_sub(1);
            Ok(reader.g8()? as i64)
        }
    }
    pub(super) fn decode_clan_settings(
        reader: &mut crate::server_prot::PayloadReader<'_>,
    ) -> anyhow::Result<ClanSettingsState> {
        let version = reader.g1()?;
        anyhow::ensure!((1..=6).contains(&version), "CLANSETTINGS_FULL version");
        let flags = reader.g1()?;
        let mut state = ClanSettingsState {
            use_user_hashes: flags & 1 != 0,
            use_display_names: flags & 2 != 0,
            update_num: reader.g4s()?,
            ..ClanSettingsState::default()
        };
        let _field = reader.g4s()?;
        let member_count = usize::from(reader.g2()?);
        let banned_count = usize::from(reader.g1()?);
        state.clan_name = reader.gjstr()?;
        if version >= 4 {
            let _ = reader.g4s()?;
        }
        state.allow_unaffined = reader.g1()? == 1;
        state.rank_talk = i32::from(reader.g1()? as i8);
        state.rank_kick = i32::from(reader.g1()? as i8);
        state.rank_lootshare = i32::from(reader.g1()? as i8);
        state.coinshare = i32::from(reader.g1()? as i8);
        state.members.reserve(member_count);
        for _ in 0..member_count {
            let hash = if state.use_user_hashes {
                reader.g8()? as i64
            } else {
                -1
            };
            let display_name = if state.use_display_names {
                reader.fastgstr()?.unwrap_or_default()
            } else {
                String::new()
            };
            let rank = i32::from(reader.g1()? as i8);
            let extra = if version >= 2 { reader.g4s()? } else { 0 };
            let joined_runedays = if version >= 5 {
                i32::from(reader.g2()?)
            } else {
                0
            };
            let muted = if version >= 6 {
                reader.g1()? == 1
            } else {
                false
            };
            state.members.push(ClanSettingsMember {
                hash,
                display_name,
                rank,
                extra,
                joined_runedays,
                muted,
            });
        }
        state.banned.reserve(banned_count);
        for _ in 0..banned_count {
            if state.use_user_hashes {
                let _ = reader.g8()?;
            }
            state.banned.push(if state.use_display_names {
                reader.fastgstr()?.unwrap_or_default()
            } else {
                String::new()
            });
        }
        if version >= 3 {
            let mut count = usize::from(reader.g2()?);
            while count > 0 {
                count -= 1;
                let raw = reader.g4s()? as u32;
                let id = (raw & 0x3fff_ffff) as i32;
                match raw >> 30 {
                    0 => {
                        state
                            .settings
                            .insert(id, ClanSettingValue::Int(reader.g4s()?));
                    }
                    1 => {
                        state
                            .settings
                            .insert(id, ClanSettingValue::Long(reader.g8()? as i64));
                    }
                    2 => {
                        state
                            .settings
                            .insert(id, ClanSettingValue::String(reader.gjstr()?));
                    }
                    // Other value types are ignored.
                    _ => {}
                }
            }
        }
        state.recompute_owner();
        Ok(state)
    }
    pub fn apply_clan_settings_full(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let affined = reader.g1()? == 1;
        let settings = if reader.remaining() == 0 {
            None
        } else {
            Some(Self::decode_clan_settings(&mut reader)?)
        };
        reader.finish("CLANSETTINGS_FULL")?;
        // Stamps the clan-settings transmit redraw cycle.
        self.stamps.clan_settings = true;
        if affined {
            self.affined_settings = settings;
        } else {
            self.listened_settings = settings;
        }
        self.active_settings = if self.active_settings_affined {
            self.affined_settings.clone()
        } else {
            self.listened_settings.clone()
        };
        self.publish_active_settings();
        Ok(())
    }
    /// Refresh the installed `CLAN_SETTING` domain from `activeClanSettings`.
    pub(super) fn publish_active_settings(&mut self) {
        *self
            .active_settings_domain
            .lock()
            .expect("clan settings domain lock") =
            self.active_settings.as_ref().map(|s| s.settings.clone());
    }
    pub(super) fn setting_int(settings: &ClanSettingsState, id: i32) -> i32 {
        match settings.settings.get(&id) {
            Some(ClanSettingValue::Int(value)) => *value,
            _ => 0,
        }
    }
    pub fn apply_clan_settings_delta(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let affined = reader.g1()? == 1;
        let owner = reader.g8()? as i64;
        let update_num = reader.g4s()?;
        let settings = if affined {
            self.affined_settings.as_mut()
        } else {
            self.listened_settings.as_mut()
        }
        .ok_or_else(|| anyhow::anyhow!("CLANSETTINGS_DELTA without settings"))?;
        anyhow::ensure!(
            settings.owner == owner && settings.update_num == update_num,
            "CLANSETTINGS_DELTA sequence mismatch"
        );
        loop {
            let opcode = reader.g1()?;
            match opcode {
                0 => break,
                1 | 13 => {
                    let hash = Self::optional_hash(&mut reader)?;
                    let display_name = reader.fastgstr()?.unwrap_or_default();
                    let joined_runedays = if opcode == 13 {
                        i32::from(reader.g2()?)
                    } else {
                        0
                    };
                    // The first member becomes owner (126), no owner rescan.
                    let rank = if settings.current_owner_slot == -1 {
                        settings.current_owner_slot = settings.members.len() as i32;
                        126
                    } else {
                        0
                    };
                    settings.members.push(ClanSettingsMember {
                        hash,
                        display_name,
                        rank,
                        joined_runedays,
                        ..ClanSettingsMember::default()
                    });
                }
                3 => {
                    let hash = Self::optional_hash(&mut reader)?;
                    let display_name = reader.fastgstr()?.unwrap_or_default();
                    let _ = hash;
                    settings.banned.push(display_name);
                }
                4 => {
                    settings.allow_unaffined = reader.g1()? == 1;
                    settings.rank_talk = i32::from(reader.g1()? as i8);
                    settings.rank_kick = i32::from(reader.g1()? as i8);
                    settings.rank_lootshare = i32::from(reader.g1()? as i8);
                    settings.coinshare = i32::from(reader.g1()? as i8);
                }
                5 => {
                    let index = usize::from(reader.g2()?);
                    anyhow::ensure!(
                        index < settings.members.len(),
                        "CLANSETTINGS_DELTA member index"
                    );
                    settings.members.remove(index);
                    settings.recompute_owner();
                }
                6 => {
                    let index = usize::from(reader.g2()?);
                    anyhow::ensure!(
                        index < settings.banned.len(),
                        "CLANSETTINGS_DELTA banned index"
                    );
                    settings.banned.remove(index);
                }
                2 => {
                    let index = usize::from(reader.g2()?);
                    let rank = i32::from(reader.g1()? as i8);
                    anyhow::ensure!(
                        index < settings.members.len(),
                        "CLANSETTINGS_DELTA member index"
                    );
                    // Setting a rank may lock the owner slot.
                    let owner_locked = settings.current_owner_slot == index as i32
                        && (settings.replacement_owner_slot == -1
                            || settings.members[settings.replacement_owner_slot as usize].rank
                                < 125);
                    if rank != 126
                        && rank != 127
                        && !owner_locked
                        && settings.members[index].rank != rank
                    {
                        settings.members[index].rank = rank;
                        settings.recompute_owner();
                    }
                }
                7 => {
                    let index = usize::from(reader.g2()?);
                    let value = reader.g4s()?;
                    let start = i32::from(reader.g1()?);
                    let end = i32::from(reader.g1()?);
                    anyhow::ensure!(
                        index < settings.members.len(),
                        "CLANSETTINGS_DELTA member index"
                    );
                    let low = (1_i32 << start).wrapping_sub(1);
                    let high = if end == 31 {
                        -1
                    } else {
                        (1_i32 << (end + 1)).wrapping_sub(1)
                    };
                    let mask = high ^ low;
                    settings.members[index].extra =
                        (settings.members[index].extra & !mask) | ((value << start) & mask);
                }
                14 => {
                    let index = usize::from(reader.g2()?);
                    let muted = reader.g1()? == 1;
                    anyhow::ensure!(
                        index < settings.members.len(),
                        "CLANSETTINGS_DELTA member index"
                    );
                    settings.members[index].muted = muted;
                }
                8 => {
                    let id = reader.g4s()?;
                    let value = reader.g4s()?;
                    settings.settings.insert(id, ClanSettingValue::Int(value));
                }
                9 => {
                    let id = reader.g4s()?;
                    let value = reader.g8()? as i64;
                    settings.settings.insert(id, ClanSettingValue::Long(value));
                }
                10 => {
                    let id = reader.g4s()?;
                    let mut value = reader.gjstr()?;
                    // Extra string settings are capped at 80 UTF-16 units.
                    if value.encode_utf16().count() > 80 {
                        value = String::from_utf16_lossy(
                            &value.encode_utf16().take(80).collect::<Vec<_>>(),
                        );
                    }
                    settings
                        .settings
                        .insert(id, ClanSettingValue::String(value));
                }
                11 => {
                    let id = reader.g4s()?;
                    let value = reader.g4s()?;
                    let start = i32::from(reader.g1()?);
                    let end = i32::from(reader.g1()?);
                    let old = Self::setting_int(settings, id);
                    let low = (1_i32 << start).wrapping_sub(1);
                    let high = if end == 31 {
                        -1
                    } else {
                        (1_i32 << (end + 1)).wrapping_sub(1)
                    };
                    let mask = high ^ low;
                    settings.settings.insert(
                        id,
                        ClanSettingValue::Int((old & !mask) | ((value << start) & mask)),
                    );
                }
                12 => {
                    settings.clan_name = reader.gjstr()?;
                    let _ = reader.g4s()?;
                }
                _ => anyhow::bail!("CLANSETTINGS_DELTA unknown entry {opcode}"),
            }
        }
        reader.finish("CLANSETTINGS_DELTA")?;
        settings.update_num = settings.update_num.wrapping_add(1);
        // Stamps the clan-settings transmit redraw cycle.
        self.stamps.clan_settings = true;
        self.active_settings = if self.active_settings_affined {
            self.affined_settings.clone()
        } else {
            self.listened_settings.clone()
        };
        self.publish_active_settings();
        Ok(())
    }
}
