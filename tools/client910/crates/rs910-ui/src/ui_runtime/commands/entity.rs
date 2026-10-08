//! The active-entity queries (`activeEntity`,
//! `get_entity_*`/`get_npc_*`/`npc_type`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::ActiveEntity;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_get_entity_screen_position(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let height = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as f32;
        let Some(frame) = self.scene.player_picks.as_ref() else {
            ints.extend([-1, -1, -1]);
            return Ok(None);
        };
        let position = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Player { position, .. })
            | Some(ActiveEntity::Npc { position, .. })
            | Some(ActiveEntity::Loc { position, .. })
            | Some(ActiveEntity::Obj { position, .. }) => *position,
            _ => {
                ints.extend([-1, -1, -1]);
                return Ok(None);
            }
        };
        let clip = crate::ui_scene_options::transform(
            &frame.vp,
            position[0],
            position[1] - height,
            position[2],
        );
        if !clip[3].is_finite() || clip[3] <= 0.0 {
            ints.extend([-1, -1, -1]);
        } else {
            ints.push((frame.screen[0] + frame.screen[2] * clip[0] / clip[3]) as i32);
            ints.push((frame.screen[1] + frame.screen[3] * clip[1] / clip[3]) as i32);
            ints.push((clip[2] / clip[3]) as i32);
        }
        Ok(None)
    }

    pub(super) fn cmd_get_entity_bounding_box(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let bounds = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Player { screen_bounds, .. })
            | Some(ActiveEntity::Npc { screen_bounds, .. })
            | Some(ActiveEntity::Loc { screen_bounds, .. })
            | Some(ActiveEntity::Obj { screen_bounds, .. }) => *screen_bounds,
            _ => None,
        };
        if let Some(bounds) = bounds {
            let min_x = bounds.a[0].min(bounds.b[0]) - bounds.radius;
            let min_y = bounds.a[1].min(bounds.b[1]) - bounds.radius;
            let max_x = bounds.a[0].max(bounds.b[0]) + bounds.radius;
            let max_y = bounds.a[1].max(bounds.b[1]) + bounds.radius;
            ints.extend([1, min_x, min_y, max_x, max_y]);
        } else {
            ints.extend([0, 0, 0, 0, 0]);
        }
        Ok(None)
    }

    pub(super) fn cmd_get_entity_say(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let text = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Player { chat, .. }) | Some(ActiveEntity::Npc { chat, .. }) => {
                chat.clone().unwrap_or_default()
            }
            _ => String::new(),
        };
        objs.push(text);
        Ok(None)
    }

    pub(super) fn cmd_get_entity_overlay_height(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let height = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Player { overlay_height, .. })
            | Some(ActiveEntity::Npc { overlay_height, .. })
            | Some(ActiveEntity::Loc { overlay_height, .. })
            | Some(ActiveEntity::Obj { overlay_height, .. }) => *overlay_height,
            None => 0,
        };
        ints.push(height);
        Ok(None)
    }

    pub(super) fn cmd_is_targeted_entity(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let target = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Player { target, .. }) | Some(ActiveEntity::Npc { target, .. }) => {
                *target
            }
            _ => -1,
        };
        Ok(Some(Value::Int(i32::from(
            self.scene.active_target == target && target != -1,
        ))))
    }

    pub(super) fn cmd_get_npc_name(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let name = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Npc { name, .. }) => name.clone(),
            _ => String::new(),
        };
        objs.push(name);
        Ok(None)
    }

    // get_npc_stat reads the active NPC's
    // current and maximum stat arrays in that order.
    pub(super) fn cmd_get_npc_stat(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let stat = usize::try_from(ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?)
            .map_err(|_| VmError::TrapFailed {
                command: c.command.into(),
                reason: "negative NPC stat index".into(),
            })?;
        let Some(ActiveEntity::Npc {
            stats, stat_max, ..
        }) = self.scene.active_entity.as_ref()
        else {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "active entity is not an NPC".into(),
            });
        };
        let current = *stats.get(stat).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "NPC stat index outside array".into(),
        })?;
        let maximum = *stat_max.get(stat).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "NPC stat index outside array".into(),
        })?;
        ints.extend([current, maximum]);
        Ok(None)
    }

    pub(super) fn cmd_get_npc_vislevel(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Npc { vislevel, .. }) => *vislevel,
            _ => 0,
        };
        ints.push(value);
        Ok(None)
    }

    pub(super) fn cmd_npc_type(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Npc { type_id, .. }) => *type_id,
            _ => -1,
        };
        ints.push(value);
        Ok(None)
    }

    pub(super) fn cmd_is_npc(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = matches!(
            self.scene.active_entity.as_ref(),
            Some(ActiveEntity::Npc { active: true, .. })
        );
        Ok(Some(Value::Int(i32::from(value))))
    }
}
