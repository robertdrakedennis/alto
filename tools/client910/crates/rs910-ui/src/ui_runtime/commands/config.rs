//! Config-type queries (`oc_*`, `nc_param`, `lc_param`, `seq_param`,
//! `inv_totalparam*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;
use rs910_core::fault::Fault;

impl Engine {
    // oc_category (category, opcode 94, default -1).
    pub(super) fn cmd_oc_category(
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
        let category = u32::try_from(id)
            .ok()
            .and_then(|id| objs.get(id))
            .map_or(-1, |o| o.category);
        Ok(Some(Value::Int(category)))
    }

    // oc_members (members, opcode 16).
    pub(super) fn cmd_oc_members(
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
        let members = u32::try_from(id)
            .ok()
            .and_then(|id| objs.get(id))
            .is_some_and(|o| o.members);
        Ok(Some(Value::Int(i32::from(members))))
    }

    // inv_totalparam -> getParamCount
    // (inv, param, false, false): one param value per occupied slot.
    pub(super) fn cmd_inv_totalparam(
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
        let (inv, param) = (a[0], a[1]);
        let Some(inventory) = self.inv_cache.inventory(inv, false) else {
            return Ok(Some(Value::Int(0)));
        };
        let objs = self
            .configs
            .objs
            .as_ref()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "obj types not installed".into(),
            })?;
        // defaultint; an absent param type decodes to the default (0).
        let default = self
            .configs
            .params
            .get(&param)
            .and_then(|p| p.default_int)
            .unwrap_or(0);
        // postDecode drops params whose autodisable
        // is set (the default for an absent type).
        let enabled = self
            .configs
            .params
            .get(&param)
            .is_some_and(|p| !p.autodisable);
        let mut total = 0i32;
        for (slot, &obj) in inventory.obj_ids.iter().enumerate() {
            // `>= 0 && < objTypeList.num`: ids past the store are skipped.
            let Some(o) = u32::try_from(obj).ok().and_then(|id| objs.get(id)) else {
                continue;
            };
            // A string entry is a wrong-value-type fault.
            let value = match o
                .params
                .iter()
                .find(|(k, _)| *k == param)
                .filter(|_| enabled || !o.members || objs.allow_members.get())
            {
                None => default,
                Some((_, crate::config::ParamValue::Int(v))) => *v,
                Some((_, crate::config::ParamValue::Str(_))) => {
                    return Err(VmError::TrapFailed {
                        command: c.command.into(),
                        reason: Fault::WrongValueType
                            .message(format_args!("obj {obj} param {param} is a string")),
                    })
                }
            };
            let count = inventory.counts.get(slot).copied().unwrap_or(0);
            if c.command == "inv_totalparam_stack" {
                total = total.wrapping_add(value.wrapping_mul(count));
            } else if count > 0 {
                total = total.wrapping_add(value);
            }
        }
        Ok(Some(Value::Int(total)))
    }

    // nc_param. The stack carries NPC id then param id; resolve the NPC
    // type's opcode-249 override before the param type default.
    pub(super) fn cmd_nc_param(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let param = ints.pop().unwrap();
        let npc_id = ints.pop().unwrap();
        if let Some(value) = match u32::try_from(npc_id) {
            Ok(id) => self
                .configs
                .npcs
                .as_ref()
                .and_then(|store| store.get(id))
                .and_then(|npc| npc.params.iter().find(|(id, _)| *id == param))
                .map(|(_, value)| value.clone()),
            Err(_) => None,
        } {
            return Ok(Some(match value {
                crate::config::ParamValue::Int(value) => Value::Int(value),
                crate::config::ParamValue::Str(value) => Value::Str(value),
            }));
        }
        let Some(entry) = self.configs.params.get(&param).cloned() else {
            *self.unsupported.entry(c.command.into()).or_default() += 1;
            return Err(absent(c.command));
        };
        let is_string = entry.kind == Some(36) || entry.kind_legacy == Some(b's');
        if is_string {
            return Ok(Some(Value::Str(entry.default_string.unwrap_or_default())));
        }
        Ok(Some(Value::Int(entry.default_int.unwrap_or(0))))
    }

    pub(super) fn cmd_lc_param(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let param = ints.pop().unwrap();
        let type_id = ints.pop().unwrap();
        let override_value = u32::try_from(type_id).ok().and_then(|id| {
            let params = match c.command {
                "oc_param" => self.configs.objs.as_ref()?.get(id)?.params.as_slice(),
                "lc_param" => self.configs.locs.as_ref()?.get(id)?.params.as_slice(),
                _ => unreachable!(),
            };
            params
                .iter()
                .find(|(key, _)| *key == param)
                .map(|(_, value)| value.clone())
        });
        if let Some(value) = override_value {
            return Ok(Some(match value {
                crate::config::ParamValue::Int(value) => Value::Int(value),
                crate::config::ParamValue::Str(value) => Value::Str(value),
            }));
        }
        let Some(entry) = self.configs.params.get(&param).cloned() else {
            *self.unsupported.entry(c.command.into()).or_default() += 1;
            return Err(absent(c.command));
        };
        if entry.kind == Some(36) || entry.kind_legacy == Some(b's') {
            return Ok(Some(Value::Str(entry.default_string.unwrap_or_default())));
        }
        Ok(Some(Value::Int(entry.default_int.unwrap_or(0))))
    }

    pub(super) fn cmd_seq_param(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let param = ints.pop().unwrap();
        let seq_id = ints.pop().unwrap();
        let override_value = u32::try_from(seq_id)
            .ok()
            .and_then(|id| self.configs.seqs.as_ref()?.get(id))
            .and_then(|seq| seq.params.iter().find(|(key, _)| *key == param))
            .map(|(_, value)| value.clone());
        if let Some(value) = override_value {
            return Ok(Some(match value {
                crate::config::ParamValue::Int(value) => Value::Int(value),
                crate::config::ParamValue::Str(value) => Value::Str(value),
            }));
        }
        let Some(entry) = self.configs.params.get(&param).cloned() else {
            *self.unsupported.entry(c.command.into()).or_default() += 1;
            return Err(absent(c.command));
        };
        if entry.kind == Some(36) || entry.kind_legacy == Some(b's') {
            return Ok(Some(Value::Str(entry.default_string.unwrap_or_default())));
        }
        Ok(Some(Value::Int(entry.default_int.unwrap_or(0))))
    }
}
