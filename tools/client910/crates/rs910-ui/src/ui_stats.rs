//! The local player's skill state: the skill defaults (archive group
//! `SKILL` = 9), the experience table, skills and stats
//! and the login reset of the session state.
//! `UPDATE_STAT` (server packet 33) is the live post-login mutation; fresh values
//! below are installed by the login reset before feed updates arrive.
use crate::{cache::Pack, ui_bytes::Cursor};
use anyhow::{anyhow, Context, Result};
use rs910_core::fault::Fault;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct XpTable {
    pub table: Vec<i32>,
}
impl XpTable {
    pub fn default_table() -> Self {
        let mut table = vec![0; 120];
        let mut sum = 0i32;
        for (i, slot) in table.iter_mut().enumerate() {
            let level = (i + 1) as i32;
            let step = (f64::from(level) + (2.0f64).powf(f64::from(level) / 7.0) * 300.0) as i32;
            sum = sum.wrapping_add(step);
            *slot = sum / 4;
        }
        Self { table }
    }
    /// Builds the table and verifies it is increasing.
    pub fn new(table: Vec<i32>) -> Result<Self> {
        for i in 1..table.len() {
            anyhow::ensure!(table[i - 1] >= 0, "Negative XP at pos:{}", i - 1);
            anyhow::ensure!(table[i] >= table[i - 1], "XP goes backwards at pos:{i}");
        }
        Ok(Self { table })
    }
    pub fn get_level(&self, xp: i32) -> i32 {
        let mut level = 0;
        for (i, &threshold) in self.table.iter().enumerate() {
            if xp < threshold {
                break;
            }
            level = i as i32 + 1;
        }
        level
    }
    pub fn get_xp(&self, level: i32) -> i32 {
        if level < 1 {
            return 0;
        }
        let level = level.min(self.table.len() as i32);
        self.table[level as usize - 1]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skill {
    pub id: i32,
    pub max_level: i32,
    pub members: bool,
    pub capped_xp: i32,
    pub capped_level: i32,
    pub table: XpTable,
    pub base_level: i32,
}
impl Skill {
    pub fn new(
        id: i32,
        max_level: i32,
        members: bool,
        capped_level: i32,
        table: XpTable,
        base_level: i32,
    ) -> Self {
        let mut s = Self {
            id,
            max_level,
            members,
            capped_xp: -1,
            capped_level: -1,
            table,
            base_level,
        };
        if members {
            s.capped_level = capped_level;
            s.capped_xp = s.get_fine_xp_from_level(capped_level);
        }
        s
    }
    pub fn is_capped(&self) -> bool {
        self.capped_xp != -1
    }
    pub fn get_level(&self, xp: i32) -> i32 {
        let level = self.table.get_level(xp) + self.base_level;
        level.min(self.max_level)
    }
    pub fn get_level_raw(&self, xp: i32) -> i32 {
        self.get_level(xp / 10)
    }
    pub fn get_xp_from_level(&self, level: i32) -> i32 {
        self.table
            .get_xp(level.min(self.max_level) - self.base_level)
    }
    pub fn get_fine_xp_from_level(&self, level: i32) -> i32 {
        self.get_xp_from_level(level).wrapping_mul(10)
    }
}

#[derive(Clone, Debug, Default)]
pub struct SkillDefaults {
    /// `skills[id]`; holes are original null entries.
    pub skills: Vec<Option<Skill>>,
    pub tables: Vec<Option<XpTable>>,
}
impl SkillDefaults {
    pub fn load(pack: &Pack) -> Result<Self> {
        let bytes = crate::js5_fetch::fetch_file(pack, "defaults", 9)?;
        Self::decode(bytes.as_deref())
    }
    /// `decode(byte[])` and `decode(Packet)`.
    pub fn decode(bytes: Option<&[u8]>) -> Result<Self> {
        let mut d = Self::default();
        let Some(bytes) = bytes else {
            return Ok(d);
        };
        let mut c = Cursor::new(bytes);
        loop {
            let opcode = c.g1()?;
            match opcode {
                0 => return Ok(d),
                1 => {
                    let count = c.g1()?;
                    let mut max_id = 0i32;
                    let mut list = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        let id = i32::from(c.g1()?);
                        let max_level = i32::from(c.g2()?);
                        let flags = c.g1()?;
                        let mut capped_level = 0;
                        let mut table = XpTable::default_table();
                        let mut base_level = 1i32;
                        let members = flags & 0x1 != 0;
                        if flags & 0x2 != 0 {
                            capped_level = i32::from(c.g1()?);
                        }
                        if flags & 0x4 != 0 {
                            let index = c.g1()? as usize;
                            table = d
                                .tables
                                .get(index)
                                .and_then(Option::as_ref)
                                .cloned()
                                .ok_or_else(|| anyhow!("skill {id} XP table {index} absent"))?;
                        }
                        if flags & 0x8 != 0 {
                            base_level = i32::from(c.g1b()?);
                        }
                        // The trailing byte is an unused flag.
                        let _flag = c.g1()? == 1;
                        list.push(Skill::new(
                            id,
                            max_level,
                            members,
                            capped_level,
                            table,
                            base_level,
                        ));
                        max_id = max_id.max(id);
                    }
                    d.skills = vec![None; max_id as usize + 1];
                    for skill in list {
                        let id = skill.id as usize;
                        d.skills[id] = Some(skill);
                    }
                }
                2 => {
                    let n = c.g1()? as usize;
                    d.tables = vec![None; n];
                    loop {
                        let index = c.g1()?;
                        if index == 255 {
                            break;
                        }
                        let len = c.g2()? as usize;
                        let mut table = Vec::with_capacity(len);
                        for _ in 0..len {
                            table.push(c.g4s()?);
                        }
                        *d.tables
                            .get_mut(index as usize)
                            .ok_or_else(|| anyhow!("XP table {index} outside {n}"))? =
                            Some(XpTable::new(table)?);
                    }
                }
                _ => {}
            }
        }
    }
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }
}

/// A stat of the local player (not the raw variant).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stat {
    pub skill: Skill,
    pub raw: bool,
    pub xp: i32,
    pub xp_level: i32,
    pub level: i32,
}
impl Stat {
    pub fn new(skill: Skill, raw: bool) -> Self {
        Self {
            skill,
            raw,
            xp: 0,
            xp_level: 1,
            level: 1,
        }
    }
    pub fn capped_xp(&self, members_account: bool) -> i32 {
        if !members_account && self.skill.members && self.skill.is_capped() {
            let mut cap = self.skill.capped_xp;
            if !self.raw {
                cap /= 10;
            }
            if self.xp > cap {
                return cap;
            }
        }
        self.xp
    }
    pub fn set_xp(&mut self, xp: i32) {
        self.xp = xp;
        if self.xp < 0 {
            self.xp = 0;
        } else if self.raw && self.xp > 2_000_000_000 {
            self.xp = 2_000_000_000;
        } else if !self.raw && self.xp > 200_000_000 {
            self.xp = 200_000_000;
        }
        self.recalculate_xp_level();
    }
    pub fn capped_xp_level(&self, members_account: bool) -> i32 {
        if !members_account && self.skill.members && self.skill.is_capped() {
            let cap = self.skill.capped_level;
            if self.xp_level > cap {
                return cap;
            }
        }
        self.xp_level
    }
    pub fn recalculate_xp_level(&mut self) {
        self.xp_level = if self.raw {
            self.skill.get_level_raw(self.xp)
        } else {
            self.skill.get_level(self.xp)
        };
    }
}

/// The local player's stats: sized by the skill count, entries null until the
/// login reset.
pub struct PlayerStats {
    pub defaults: SkillDefaults,
    pub stats: Vec<Option<Stat>>,
    /// `loggedInMembers` selects `MEMBERS`/`FREE`.
    pub logged_in_members: bool,
}
impl PlayerStats {
    pub fn new(defaults: SkillDefaults) -> Self {
        let n = defaults.skill_count();
        Self {
            defaults,
            stats: vec![None; n],
            logged_in_members: false,
        }
    }
    pub fn load(pack: &Pack) -> Result<Self> {
        Ok(Self::new(SkillDefaults::load(pack)?))
    }
    pub fn reset_session(&mut self) -> Result<()> {
        for i in 0..self.stats.len() {
            let skill = self.defaults.skills[i]
                .clone()
                .with_context(|| Fault::MissingValue.message(format_args!("skill default {i}")))?;
            let mut stat = Stat::new(skill, false);
            stat.set_xp(0);
            stat.level = 0;
            self.stats[i] = Some(stat);
        }
        Ok(())
    }
    /// Apply `UPDATE_STAT` exactly as `read` does: the displayed level
    /// is carried independently, while `setXP` clamps XP and
    /// recalculates the actual XP level. The skill array lookup intentionally
    /// errors for invalid ids, matching the original client's array access failure.
    pub fn update_stat(&mut self, skill: u8, current_level: u8, xp: i32) -> Result<()> {
        let stat = self
            .stats
            .get_mut(skill as usize)
            .with_context(|| Fault::IndexOutOfRange.message(format_args!("stat {skill}")))?
            .as_mut()
            .with_context(|| {
                Fault::MissingValue.message(format_args!("stat {skill} before login reset"))
            })?;
        stat.set_xp(xp);
        stat.level = i32::from(current_level);
        Ok(())
    }
    fn stat(&self, index: i32) -> Result<&Stat> {
        usize::try_from(index)
            .ok()
            .and_then(|i| self.stats.get(i))
            .with_context(|| Fault::IndexOutOfRange.message(format_args!("stat {index}")))?
            .as_ref()
            .with_context(|| {
                Fault::MissingValue.message(format_args!("stat {index} before login reset"))
            })
    }
    pub fn stat_xp(&self, i: i32) -> Result<i32> {
        Ok(self.stat(i)?.capped_xp(self.logged_in_members))
    }
    pub fn stat_level(&self, i: i32) -> Result<i32> {
        Ok(self.stat(i)?.level)
    }
    pub fn stat_level_max(&self, i: i32) -> Result<i32> {
        Ok(self.stat(i)?.capped_xp_level(self.logged_in_members))
    }
    pub fn stat_xp_actual(&self, i: i32) -> Result<i32> {
        Ok(self.stat(i)?.xp)
    }
    pub fn stat_level_max_actual(&self, i: i32) -> Result<i32> {
        Ok(self.stat(i)?.xp_level)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_table_matches_the_first_levels() {
        let t = XpTable::default_table();
        assert_eq!(&t.table[..4], &[83, 174, 276, 388]);
        assert_eq!(t.get_level(0), 0);
        assert_eq!(t.get_level(83), 1);
        assert_eq!(t.get_xp(0), 0);
        assert_eq!(t.get_xp(2), 174);
    }

    #[test]
    fn fresh_login_stats_are_level_one_xp_zero() {
        let d = SkillDefaults {
            skills: vec![Some(Skill::new(
                0,
                99,
                false,
                0,
                XpTable::default_table(),
                1,
            ))],
            tables: vec![],
        };
        let mut s = PlayerStats::new(d);
        assert!(s.stat_level_max(0).is_err());
        s.reset_session().unwrap();
        assert_eq!(s.stat_level_max(0).unwrap(), 1);
        assert_eq!(s.stat_xp_actual(0).unwrap(), 0);
        assert_eq!(s.stat_level(0).unwrap(), 0);
        assert!(s.stat_level_max(1).is_err());
    }

    #[test]
    fn update_stat_clamps_xp_and_keeps_wire_level() {
        let d = SkillDefaults {
            skills: vec![Some(Skill::new(
                0,
                99,
                false,
                0,
                XpTable::default_table(),
                1,
            ))],
            tables: vec![],
        };
        let mut s = PlayerStats::new(d);
        s.reset_session().unwrap();
        s.update_stat(0, 77, i32::MAX).unwrap();
        assert_eq!(s.stat_level(0).unwrap(), 77);
        assert_eq!(s.stat_xp_actual(0).unwrap(), 200_000_000);
        s.update_stat(0, 4, -1).unwrap();
        assert_eq!(s.stat_level(0).unwrap(), 4);
        assert_eq!(s.stat_xp_actual(0).unwrap(), 0);
        // An undefined skill fails like the original client's array access.
        assert!(s.update_stat(1, 1, 0).is_err());
    }
}
