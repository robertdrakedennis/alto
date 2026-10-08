//! Deterministic host for CS2 fixtures. Definitions are explicit; unsupported
//! engine behavior fails. This is not a replacement for the game's event loop.
use crate::vars::VarScope;
use crate::vm::{Host, Value, VarLane, VmError, VmResult};
use std::collections::HashMap;

#[derive(Default)]
pub struct RuntimeHost {
    pub definitions: HashMap<(VarScope, u16), VarLane>,
    pub variables: HashMap<(VarScope, u16, bool), Value>,
    pub arrays: HashMap<i32, Vec<i32>>,
    /// (packed parent, runtime child index); -1 identifies a packed component.
    pub component_text: HashMap<(i32, i32), String>,
    pub active_components: [Option<(i32, i32)>; 2],
    scope: usize,
    effects: Vec<crate::vm::HostEffect>,
}

impl Host for RuntimeHost {
    fn take_effects(&mut self) -> Vec<crate::vm::HostEffect> {
        std::mem::take(&mut self.effects)
    }
    fn trap_context(
        &mut self,
        context: &crate::vm::InstructionContext<'_>,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.scope = usize::from(context.secondary);
        crate::vm::dispatch_checked(self, context, ints, strs, longs)
    }
    fn trap(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        strs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let pop =
            |values: &mut Vec<i32>| values.pop().ok_or(VmError::StackUnderflow { stack: "int" });
        match command {
            // The original handler always pushes zero and consumes nothing.
            "detailcanset_vsync" => Ok(Some(Value::Int(0))),
            "cc_find" | "if_find" => {
                let child = if command == "cc_find" { pop(ints)? } else { -1 };
                let packed = pop(ints)?;
                // cc_find short-circuits on -1 without changing active scope.
                if command == "cc_find" && child == -1 {
                    return Ok(Some(Value::Int(0)));
                }
                let key = (packed, child);
                let found = self.component_text.contains_key(&key);
                self.active_components[self.scope] = found.then_some(key);
                Ok(Some(Value::Int(i32::from(found))))
            }
            "cc_settext" | "if_settext" | "cc_gettext" | "if_gettext" => {
                let key = if command.starts_with("if_") {
                    (pop(ints)?, -1)
                } else {
                    self.active_components[self.scope].ok_or_else(|| VmError::UnknownCommand {
                        command: "missing active component".into(),
                    })?
                };
                let text =
                    self.component_text
                        .get_mut(&key)
                        .ok_or_else(|| VmError::UnknownCommand {
                            command: format!("missing component {key:?}"),
                        })?;
                if command.ends_with("settext") {
                    let value = strs
                        .pop()
                        .ok_or(VmError::StackUnderflow { stack: "object" })?;
                    if *text != value {
                        text.clone_from(&value);
                        self.effects.push(crate::vm::HostEffect::ComponentText {
                            packed: key.0,
                            child: key.1,
                            text: value,
                        });
                    }
                    Ok(None)
                } else {
                    Ok(Some(Value::Str(text.clone())))
                }
            }
            _ => Err(VmError::UnknownCommand {
                command: command.into(),
            }),
        }
    }

    fn var_type(&mut self, domain: VarScope, id: u16) -> VmResult<VarLane> {
        self.definitions
            .get(&(domain, id))
            .copied()
            .ok_or_else(|| VmError::UnknownCommand {
                command: format!("undefined variable {domain:?}:{id}"),
            })
    }
    fn var_integer_default(&mut self, domain: VarScope, id: u16) -> VmResult<i32> {
        if self.var_type(domain, id)? != VarLane::Int {
            return Err(VmError::BadOperand {
                command: "variable default".into(),
                expected: "integer definition",
            });
        }
        Ok(i32::default())
    }
    fn var_get(&mut self, domain: VarScope, id: u16, secondary: bool) -> VmResult<Value> {
        let kind = self.var_type(domain, id)?;
        Ok(self
            .variables
            .get(&(domain, id, secondary))
            .cloned()
            .unwrap_or(match kind {
                VarLane::Int => Value::Int(0),
                VarLane::String => Value::Str(String::new()),
                VarLane::Long => Value::Long(0),
            }))
    }
    fn var_set(
        &mut self,
        domain: VarScope,
        id: u16,
        secondary: bool,
        value: Value,
    ) -> VmResult<()> {
        let lane = self.var_type(domain, id)?;
        if !matches!(
            (lane, &value),
            (VarLane::Int, Value::Int(_))
                | (VarLane::String, Value::Str(_) | Value::Null)
                | (VarLane::Long, Value::Long(_))
        ) {
            return Err(VmError::BadOperand {
                command: "fixture variable write".into(),
                expected: "value matching variable definition",
            });
        }
        self.variables
            .insert((domain, id, secondary), value.clone());
        self.effects.push(crate::vm::HostEffect::VariableWrite {
            domain,
            id,
            secondary,
            value,
        });
        Ok(())
    }
    fn varbit_get(&mut self, id: u16, _secondary: bool) -> VmResult<i32> {
        Err(VmError::UnknownCommand {
            command: format!("varbit {id}"),
        })
    }
    fn varbit_set(&mut self, id: u16, _secondary: bool, _value: i32) -> VmResult<()> {
        Err(VmError::UnknownCommand {
            command: format!("varbit {id}"),
        })
    }
    fn array_define(&mut self, id: i32, len: usize) -> VmResult<()> {
        if !(0..5).contains(&id) || len > 5000 {
            return Err(VmError::BadArray {
                id,
                reason: "definition out of bounds".to_string(),
            });
        }
        self.arrays.insert(id, vec![0; len]);
        Ok(())
    }
    fn array_len(&mut self, id: i32) -> VmResult<usize> {
        self.arrays
            .get(&id)
            .map(Vec::len)
            .ok_or_else(|| VmError::BadArray {
                id,
                reason: "undefined array".to_string(),
            })
    }
    fn array_get(&mut self, id: i32, index: i32) -> VmResult<i32> {
        self.arrays
            .get(&id)
            .and_then(|a| usize::try_from(index).ok().and_then(|i| a.get(i)))
            .copied()
            .ok_or_else(|| VmError::BadArray {
                id,
                reason: format!("index {index}"),
            })
    }
    fn array_set(&mut self, id: i32, index: i32, value: i32) -> VmResult<()> {
        let cell = self
            .arrays
            .get_mut(&id)
            .and_then(|a| usize::try_from(index).ok().and_then(|i| a.get_mut(i)))
            .ok_or_else(|| VmError::BadArray {
                id,
                reason: format!("index {index}"),
            })?;
        *cell = value;
        Ok(())
    }
}
