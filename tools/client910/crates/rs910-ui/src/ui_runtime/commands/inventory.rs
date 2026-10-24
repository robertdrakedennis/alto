//! Inventory cache queries (`inv_*`, `invother_*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::Engine;
use anyhow::Result;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    // Queries over the inventory cache
    // (ui_inv.rs), fed by UPDATE_INV_* packets.
    pub(super) fn cmd_inv_getobj(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let a = ints.split_off(ints.len() - 2);
        let secondary = c.command.starts_with("invother_");
        let v = match c.command {
            "inv_getobj" | "invother_getobj" => self.inv_cache.slot_type(a[0], a[1], secondary),
            "inv_getnum" | "invother_getnum" => self.inv_cache.slot_count(a[0], a[1], secondary),
            _ => self.inv_cache.total(a[0], a[1], secondary),
        };
        Ok(Some(Value::Int(v)))
    }

    pub(super) fn cmd_inv_getvar(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 3 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let a = ints.split_off(ints.len() - 3);
        let value = (|| -> Result<i32> {
            let defs = self
                .configs
                .inv_varbits
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("inventory varbits not installed"))?;
            self.inv_cache
                .slot_var(a[0], a[1], a[2], c.command == "invother_getvar", defs)
        })()
        .map_err(|e| VmError::TrapFailed {
            command: c.command.into(),
            reason: format!("{e:#}"),
        })?;
        Ok(Some(Value::Int(value)))
    }

    pub(super) fn cmd_inv_freespace(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let inv = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let Some(size) = self.configs.inv_sizes.get(&inv).copied() else {
            *self.unsupported.entry(c.command.into()).or_default() += 1;
            return Err(absent(c.command));
        };
        Ok(Some(Value::Int(
            self.inv_cache.free_space(inv, false, size),
        )))
    }

    // inv_size: size from the cache config.
    pub(super) fn cmd_inv_size(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let Some(size) = self.configs.inv_sizes.get(&id).copied() else {
            *self.unsupported.entry(c.command.into()).or_default() += 1;
            return Err(absent(c.command));
        };
        Ok(Some(Value::Int(size)))
    }
}
