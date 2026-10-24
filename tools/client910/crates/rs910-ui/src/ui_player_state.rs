//! `localPlayerGameState` as scripts see it: stat queries
//! and quest queries
//! over the borrowed player varps/varbits. Dispatched before the engine so
//! the variable owner, not a copied snapshot, answers.
use crate::{
    ui_quests::{Player, QuestStore},
    ui_stats::PlayerStats,
    ui_vars::Variables,
};
use anyhow::Result;
use native910::{
    vars::VarScope,
    vm::{Value, VmError, VmResult},
};
use rs910_core::fault::Fault;

struct GameState<'a, 'b> {
    vars: &'a Variables<'b>,
    stats: Option<&'a PlayerStats>,
}
impl Player for GameState<'_, '_> {
    fn varp(&self, id: i32) -> Result<i32> {
        let id =
            u16::try_from(id).map_err(|_| anyhow::anyhow!("varp {id} outside the player list"))?;
        match self.vars.get(VarScope::Player, id, false)? {
            Value::Int(v) => Ok(v),
            other => anyhow::bail!("player var {id} is not an int: {other:?}"),
        }
    }
    fn varbit(&self, id: i32) -> Result<i32> {
        let id = u16::try_from(id).map_err(|_| anyhow::anyhow!("varbit {id} outside the list"))?;
        // The varbit type lookup fails unless the base
        // var is a player var.
        anyhow::ensure!(
            self.vars.bit_domain(id)? == 0,
            "{}",
            Fault::MissingValue.message(format_args!("varbit {id} base is not a player var"))
        );
        self.vars.get_bit(id, false)
    }
    fn stat_level_max(&self, stat: i32) -> Result<i32> {
        self.stats
            .ok_or_else(|| anyhow::anyhow!("player stats not installed"))?
            .stat_level_max(stat)
    }
}

fn failed(command: &str, e: anyhow::Error) -> VmError {
    VmError::TrapFailed {
        command: command.into(),
        reason: format!("{e:#}"),
    }
}

/// `None` when `command` is neither a stat nor a quest query.
pub fn dispatch(
    vars: &Variables<'_>,
    stats: Option<&PlayerStats>,
    quests: Option<&QuestStore>,
    command: &str,
    ints: &mut Vec<i32>,
    objs: &mut Vec<String>,
) -> Option<VmResult<Option<Value>>> {
    if command == "comlevel_active" {
        //  reads the decoded local actor's combatLevel.
        // It is independent of the skill XP/current-level presentation.
        let player = vars.scene.local_player.and_then(|local| {
            vars.scene
                .players?
                .players
                .get(local.index as usize)?
                .as_ref()
        });
        return Some(
            player
                .map(|p| Some(Value::Int(p.appearance.combat)))
                .ok_or_else(|| failed(command, anyhow::anyhow!("local player entity missing"))),
        );
    }
    type Getter = fn(&PlayerStats, i32) -> Result<i32>;
    let stat: Option<Getter> = match command {
        "stat" => Some(PlayerStats::stat_level),
        "stat_base" => Some(PlayerStats::stat_level_max),
        "stat_visible_xp" => Some(PlayerStats::stat_xp),
        "stat_base_actual" => Some(PlayerStats::stat_level_max_actual),
        "stat_visible_xp_actual" => Some(PlayerStats::stat_xp_actual),
        _ => None,
    };
    if let Some(getter) = stat {
        let stats = stats?;
        let Some(index) = ints.pop() else {
            return Some(Err(VmError::StackUnderflow { stack: "int" }));
        };
        return Some(
            getter(stats, index)
                .map(|v| Some(Value::Int(v)))
                .map_err(|e| failed(command, e)),
        );
    }
    if command.starts_with("quest_") {
        let quests = quests?;
        let state = GameState { vars, stats };
        return quests.dispatch(&state, command, ints, objs);
    }
    None
}
