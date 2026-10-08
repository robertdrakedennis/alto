//! `MiniMenu` queries and cursor defaults.
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    // The original client deliberately discards these two operands; cursor selection
    // uses the retained menu/component values, not hardcoded replacements.
    pub(super) fn cmd_sethardcodedopcursors(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        ints.truncate(ints.len() - 2);
        Ok(None)
    }

    // setdefaultcursors.
    pub(super) fn cmd_setdefaultcursors(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let a = ints.split_off(ints.len() - 2);
        self.menu.default_cursors = [a[0], a[1]];
        Ok(None)
    }

    // get_active_minimenu_entry/get_second_minimenu_entry
    // read the snapshot installed after the normal update.
    pub(super) fn cmd_get_active_minimenu_entry(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let view = if c.command == "get_active_minimenu_entry" {
            &self.menu.active
        } else {
            &self.menu.secondary
        };
        ints.push(view.entity_type);
        objs.push(view.op.clone());
        objs.push(view.op_base.clone());
        objs.push(view.quest_text.clone().unwrap_or_default());
        Ok(None)
    }

    pub(super) fn cmd_get_minimenu_length(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.extend(self.menu.counts);
        Ok(None)
    }

    pub(super) fn cmd_oc_minimenu_colour(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        self.minimenu_colour(id)
            .map(|v| Some(Value::Int(v)))
            .map_err(|e| VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("{e:#}"),
            })
    }

    // oc_minimenu_colour_overridden.
    pub(super) fn cmd_oc_minimenu_colour_overridden(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let objs = self
            .configs
            .objs
            .as_ref()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "obj types not installed".into(),
            })?;
        let overridden = u32::try_from(id)
            .ok()
            .and_then(|id| objs.get(id))
            .is_some_and(|o| o.minimenu_colour.is_some());
        Ok(Some(Value::Int(i32::from(overridden))))
    }
}
