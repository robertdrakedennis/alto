//! Account/player flags (`staffmodlevel`, `playermod*`, `playermember`,
//! `userdetail_quickchat`/`_dob`, `map_members`/`map_quickchat`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmResult;

impl Engine {
    // staffmodlevel: levels below 2 read as 0.
    pub(super) fn cmd_staffmodlevel(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(if self.account.staff_mod_level >= 2 {
            self.account.staff_mod_level
        } else {
            0
        })))
    }

    // playermod/playermodlevel. Only moderator
    // levels 5..9 are exposed by these clientscript queries.
    pub(super) fn cmd_playermod(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let level = if (5..=9).contains(&self.account.player_mod_level) {
            self.account.player_mod_level
        } else {
            0
        };
        Ok(Some(Value::Int(if c.command == "playermod" {
            i32::from(level != 0)
        } else {
            level
        })))
    }

    // userdetail_quickchat: dobVerified && !playerIsQuickChat.
    pub(super) fn cmd_userdetail_quickchat(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(
            self.account.dob_verified && !self.account.player_is_quickchat,
        ))))
    }

    // userdetail_dob.
    pub(super) fn cmd_userdetail_dob(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.account.dob)))
    }

    // map_members.
    pub(super) fn cmd_map_members(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.account.logged_in_members))))
    }

    // map_quickchat reads the login profile's
    // logged-in quick-chat flag, distinct from the DOB/player setting.
    pub(super) fn cmd_map_quickchat(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(
            self.account.logged_in_quickchat,
        ))))
    }

    pub(super) fn cmd_playermember(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.account.player_is_members))))
    }
}
