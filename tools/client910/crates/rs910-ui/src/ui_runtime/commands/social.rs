//! Social commands kept beside the `ui_social` owner: the friend/ignore/
//! clan mutations that write client packets and the varbit branch of
//! `player_group_member_get_same_world_var`.
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use anyhow::Result;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_social_mutation(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let packet = self.social.mutation(c.command, ints, objs)?;
        self.outgoing.extend(packet);
        Ok(None)
    }

    /// A group member's experience in a skill, or with the flag set the level it earns. Raw
    /// experience is in tenths, so the level comes from the skill's raw-level rule.
    pub(super) fn cmd_player_group_member_get_join_xp(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 3 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let args = ints.split_off(ints.len() - 3);
        let fail = |reason: &str| VmError::TrapFailed {
            command: c.command.into(),
            reason: reason.into(),
        };
        let group = self
            .social
            .player_group
            .as_ref()
            .ok_or_else(|| fail("player group is null"))?;
        let member = usize::try_from(args[0])
            .ok()
            .and_then(|index| group.members.get(index))
            .ok_or_else(|| fail("player group member missing"))?;
        let stat = usize::try_from(args[1]).map_err(|_| fail("negative stat index"))?;
        let defaults = self
            .configs
            .skills
            .as_ref()
            .ok_or_else(|| fail("skill definitions not installed"))?;
        // A member holds one stat per skill; the packet may carry fewer, the rest are zero.
        if stat >= defaults.skill_count() {
            return Err(fail("stat outside the skills"));
        }
        let xp = member.stats.get(stat).copied().unwrap_or(0);
        let value = if args[2] == 1 {
            defaults
                .skills
                .get(stat)
                .and_then(Option::as_ref)
                .ok_or_else(|| fail("skill has no definition"))?
                .get_level_raw(xp)
        } else {
            xp
        };
        ints.push(value);
        Ok(None)
    }

    // The varbit branch of
    // player_group_member_get_same_world_var reads the member's
    // `clearVariables()` base varp through the VarBitConfig.
    pub(super) fn cmd_player_group_member_get_same_world_var(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if c.command == "player_group_member_get_same_world_var"
            && ints.len() >= 3
            && ints[ints.len() - 2] != 1
        {
            let args = ints.split_off(ints.len() - 3);
            let value = (|| -> Result<i32> {
                let member = self
                    .social
                    .player_group
                    .as_ref()
                    .and_then(|group| group.members.get(usize::try_from(args[0]).ok()?))
                    .ok_or_else(|| anyhow::anyhow!("player group member missing"))?;
                let defs = self
                    .configs
                    .inv_varbits
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("varbit definitions not installed"))?;
                let bit = defs
                    .get(args[2], false)
                    .map_err(|error| anyhow::anyhow!("varbit {}: {error:?}", args[2]))?;
                let base = bit
                    .binding
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("varbit {} is unbound", args[2]))?;
                let raw = match member.variables.as_ref().and_then(|v| v.get(&base.id)) {
                    Some(crate::ui_vars::Value::Int(value)) => *value,
                    _ => 0,
                };
                bit.get(raw)
                    .map_err(|error| anyhow::anyhow!("varbit value: {error:?}"))
            })()
            .map_err(|e| VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("{e:#}"),
            })?;
            return Ok(Some(Value::Int(value)));
        }
        // The guard failed: the old chain went on to the owners
        // (`social` claims this name).
        self.dispatch_owners(c, ints, objs, longs)
    }
}
