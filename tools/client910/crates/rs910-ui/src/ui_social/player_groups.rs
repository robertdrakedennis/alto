//! Player-group full and delta updates and sparse variable domains.

use crate::ui_vars::Value as SparseValue;

use std::collections::BTreeMap;

use super::{PlayerGroupMemberState, PlayerGroupState, State};

impl State {
    pub fn enable_clan_vars(&mut self) {
        self.clan_vars = Some(BTreeMap::new());
    }
    pub fn disable_clan_vars(&mut self) {
        self.clan_vars = None;
    }
    /// Decode one variable-value record. The caller supplies
    /// the cache-backed base type for the sparse clan variable id; unknown or
    /// non-stack serializable types stay explicit errors instead of consuming
    /// an unbounded payload as an integer.
    pub fn apply_clan_var<F>(&mut self, bytes: &[u8], base_type: F) -> anyhow::Result<()>
    where
        F: Fn(i32) -> Option<u8>,
    {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let id = i32::from(reader.g2()?);
        let value = match base_type(id) {
            Some(0) => SparseValue::Int(reader.g4s()?),
            Some(1) => SparseValue::Long(reader.g8()? as i64),
            Some(2) => {
                anyhow::ensure!(reader.g1()? == 0, "VARCLAN string prefix");
                SparseValue::String(reader.gjstr()?.encode_utf16().collect())
            }
            Some(_) => anyhow::bail!("VARCLAN variable {id} has unsupported serializable type"),
            None => anyhow::bail!("VARCLAN variable definition {id} missing"),
        };
        reader.finish("VARCLAN")?;
        self.clan_vars
            .get_or_insert_with(BTreeMap::new)
            .insert(id, value);
        Ok(())
    }
    pub(super) fn decode_group_value<F>(
        reader: &mut crate::server_prot::PayloadReader<'_>,
        id: i32,
        type_of: &F,
    ) -> anyhow::Result<SparseValue>
    where
        F: Fn(i32) -> Option<u8>,
    {
        match type_of(id) {
            Some(0) => Ok(SparseValue::Int(reader.g4s()?)),
            Some(1) => Ok(SparseValue::Long(reader.g8()? as i64)),
            Some(2) => {
                anyhow::ensure!(reader.g1()? == 0, "player-group string prefix");
                Ok(SparseValue::String(
                    reader.gjstr()?.encode_utf16().collect(),
                ))
            }
            Some(_) => anyhow::bail!("player-group variable {id} has unsupported type"),
            None => anyhow::bail!("player-group variable definition {id} missing"),
        }
    }
    pub(super) fn decode_group_member<F>(
        reader: &mut crate::server_prot::PayloadReader<'_>,
        has_uid: bool,
        has_display_name: bool,
        type_of: &F,
    ) -> anyhow::Result<PlayerGroupMemberState>
    where
        F: Fn(i32) -> Option<u8>,
    {
        let group_uid = if has_uid { reader.g8()? as i64 } else { -1 };
        let display_name = if has_display_name {
            reader.fastgstr()?.unwrap_or_default()
        } else {
            String::new()
        };
        let member_flags = reader.g1()?;
        let stat_count = usize::from(reader.g1()?);
        let mut stats = Vec::with_capacity(stat_count);
        for _ in 0..stat_count {
            stats.push(reader.g4s()?);
        }
        let var_count = usize::from(reader.g2()?);
        let mut vars = BTreeMap::new();
        for _ in 0..var_count {
            let id = i32::from(reader.g2()?);
            vars.insert(id, Self::decode_group_value(reader, id, type_of)?);
        }
        let node_id = match reader.g2()? {
            u16::MAX => -1,
            value => i32::from(value),
        };
        let rank = i32::from(reader.g1()?);
        let status = i32::from(reader.g1()?);
        let team = i32::from(reader.g1()?);
        Ok(PlayerGroupMemberState {
            group_uid,
            display_name,
            members: member_flags & 1 != 0,
            online: member_flags & 2 != 0,
            stats,
            vars,
            variables: None,
            node_id,
            rank,
            status,
            team,
        })
    }
    pub fn apply_player_group_full<F, G>(
        &mut self,
        bytes: &[u8],
        group_type: F,
        member_type: G,
    ) -> anyhow::Result<()>
    where
        F: Fn(i32) -> Option<u8>,
        G: Fn(i32) -> Option<u8>,
    {
        if bytes.is_empty() {
            self.player_group_present = false;
            self.player_group_name.clear();
            self.player_group = None;
            return Ok(());
        }
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        anyhow::ensure!(reader.g1()? == 1, "PLAYER_GROUP_FULL version");
        let flags = reader.g1()?;
        let update_num = reader.g4s()?;
        let creation_time = reader.g8()? as i64;
        let display_name = reader.gjstr()?;
        let max_size = i32::from(reader.g2()? as i16);
        let field = reader.g4s()?;
        let _ = reader.g8()?;
        let mut group = PlayerGroupState {
            update_num,
            creation_time,
            display_name,
            members_only: flags & 1 != 0,
            has_uid: flags & 2 != 0,
            has_display_name: flags & 4 != 0,
            max_size,
            field,
            ..PlayerGroupState::default()
        };
        let member_count = usize::from(reader.g2()?);
        group.members.reserve(member_count);
        for _ in 0..member_count {
            group.members.push(Self::decode_group_member(
                &mut reader,
                group.has_uid,
                group.has_display_name,
                &member_type,
            )?);
        }
        let banned_count = usize::from(reader.g2()?);
        group.banned.reserve(banned_count);
        for _ in 0..banned_count {
            if group.has_uid {
                let _ = reader.g8()?;
            }
            group.banned.push(if group.has_display_name {
                reader.fastgstr()?.unwrap_or_default()
            } else {
                String::new()
            });
        }
        let var_count = usize::from(reader.g2()?);
        for _ in 0..var_count {
            let id = i32::from(reader.g2()?);
            group
                .vars
                .insert(id, Self::decode_group_value(&mut reader, id, &group_type)?);
        }
        reader.finish("PLAYER_GROUP_FULL")?;
        group.recompute_owner();
        self.player_group_present = true;
        self.player_group_name = group.display_name.clone();
        self.player_group = Some(group);
        Ok(())
    }
    pub fn apply_player_group_delta<F, G, H>(
        &mut self,
        bytes: &[u8],
        group_type: F,
        member_type: G,
        varbit_type: H,
    ) -> anyhow::Result<Vec<i32>>
    where
        F: Fn(i32) -> Option<u8>,
        G: Fn(i32) -> Option<u8>,
        H: Fn(i32) -> Option<(i32, i32, i32)>,
    {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let hashcode = reader.g8()? as i64;
        let update_num = reader.g4s()?;
        let group = self
            .player_group
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA without group"))?;
        anyhow::ensure!(
            group.hashcode == hashcode && group.update_num == update_num,
            "PLAYER_GROUP_DELTA sequence mismatch"
        );
        let mut changed_vars = Vec::new();
        loop {
            match reader.g1()? {
                0 => break,
                1 => {
                    let has_uid = reader.g1()? != 255;
                    if has_uid {
                        reader.pos = reader.pos.saturating_sub(1);
                    }
                    let member =
                        Self::decode_group_member(&mut reader, has_uid, true, &member_type)?;
                    group.members.push(member);
                    group.recompute_owner();
                }
                2 => {
                    let index = usize::from(reader.g2()?);
                    anyhow::ensure!(
                        index < group.members.len(),
                        "PLAYER_GROUP_DELTA member index"
                    );
                    group.members.remove(index);
                    group.recompute_owner();
                }
                3 => {
                    let has_uid = reader.g1()? != 255;
                    if has_uid {
                        reader.pos = reader.pos.saturating_sub(1);
                    }
                    if has_uid {
                        let _ = reader.g8()?;
                    }
                    group.banned.push(reader.fastgstr()?.unwrap_or_default());
                }
                4 => {
                    let index = usize::from(reader.g2()?);
                    anyhow::ensure!(
                        index < group.banned.len(),
                        "PLAYER_GROUP_DELTA banned index"
                    );
                    group.banned.remove(index);
                }
                5 => {
                    let index = usize::from(reader.g2()?);
                    let rank = i32::from(reader.g1()?);
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    member.rank = rank;
                    group.recompute_owner();
                }
                6 => {
                    let index = usize::from(reader.g2()?);
                    let node_id = match reader.g2()? {
                        u16::MAX => -1,
                        value => i32::from(value),
                    };
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    member.node_id = node_id;
                    member.online = true;
                }
                7 => {
                    let index = usize::from(reader.g2()?);
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    member.online = false;
                }
                8 => {
                    let index = usize::from(reader.g2()?);
                    let loading = reader.g1()? == 1;
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    // Member status: NOT_READY (serial 1) or TELEPORTED (serial 0).
                    member.status = if loading { 1 } else { 0 };
                }
                9 => {
                    for member in &mut group.members {
                        member.status = 2;
                    }
                }
                10 => {
                    // All members become READY (serial 3).
                    for member in &mut group.members {
                        member.status = 3;
                    }
                }
                11 => {
                    let index = usize::from(reader.g2()?);
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    let updated =
                        Self::decode_group_member(&mut reader, false, false, &member_type)?;
                    member.stats = updated.stats;
                    member.vars = updated.vars;
                    member.members = updated.members;
                }
                12 => {
                    let id = reader.g2()?;
                    if id != u16::MAX {
                        let id = i32::from(id);
                        group
                            .vars
                            .insert(id, Self::decode_group_value(&mut reader, id, &group_type)?);
                        changed_vars.push(id);
                    }
                }
                13 => {
                    // GroupRosterDelta SetVarbitValue: the value follows only
                    // a real varbit id.
                    let id = i32::from(reader.g2()?);
                    if id != u16::MAX as i32 {
                        let value = reader.g4s()?;
                        if let Some((base_id, start, end)) = varbit_type(id) {
                            let old = match group.vars.get(&base_id) {
                                Some(SparseValue::Int(v)) => *v,
                                _ => 0,
                            };
                            let width = end - start;
                            if (0..32).contains(&width)
                                && value >= 0
                                && (width == 31 || value <= ((1_i32 << (width + 1)) - 1))
                            {
                                let mask = if width == 31 {
                                    -1
                                } else {
                                    (1_i32 << (width + 1)) - 1
                                } << start;
                                group.vars.insert(
                                    base_id,
                                    SparseValue::Int((old & !mask) | ((value << start) & mask)),
                                );
                                changed_vars.push(base_id);
                            }
                        }
                    }
                }
                14 => {
                    let index = usize::from(reader.g2()?);
                    let team = i32::from(reader.g1()?);
                    let member = group
                        .members
                        .get_mut(index)
                        .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_DELTA member index"))?;
                    member.team = team;
                }
                _ => anyhow::bail!("PLAYER_GROUP_DELTA unknown entry"),
            }
        }
        reader.finish("PLAYER_GROUP_DELTA")?;
        group.update_num = group.update_num.wrapping_add(1);
        Ok(changed_vars)
    }
    pub fn apply_player_group_vars<F>(
        &mut self,
        bytes: &[u8],
        member_type: F,
    ) -> anyhow::Result<usize>
    where
        F: Fn(i32) -> Option<u8>,
    {
        let mut reader = crate::server_prot::PayloadReader::new(bytes);
        let index = usize::from(reader.g2()?);
        let reset = reader.g1()? == 1;
        let group = self
            .player_group
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_VARPS without group"))?;
        let member = group
            .members
            .get_mut(index)
            .ok_or_else(|| anyhow::anyhow!("PLAYER_GROUP_VARPS member index"))?;
        // A missing or reset container is recreated.
        if reset || member.variables.is_none() {
            member.variables = Some(BTreeMap::new());
        }
        let variables = member.variables.as_mut().expect("installed above");
        let mut changed = 0;
        while reader.remaining() > 0 {
            let id = i32::from(reader.g2()?);
            if let Some(base) = member_type(id) {
                self.group_var_types.insert(id, base);
            }
            variables.insert(id, Self::decode_group_value(&mut reader, id, &member_type)?);
            changed += 1;
        }
        reader.finish("PLAYER_GROUP_VARPS")?;
        Ok(changed)
    }
}
