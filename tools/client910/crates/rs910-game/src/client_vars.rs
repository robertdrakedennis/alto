//! Sparse client variable values, lifetimes and reset.
//! Split out of client910's `ui_vars` (Phase 2.6), which re-exports it;
//! `ui_var_store` persists it and the CS2 host's `ui_vars::Variables` reads
//! it.
use crate::protocol910::{varbits::Binding, variables::Value as WireValue};
use native910::vm::Value as VmValue;
use std::collections::{BTreeMap, BTreeSet};

/// Strings may contain lone surrogates, including after truncation to 80 chars.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i32),
    Long(i64),
    String(Vec<u16>),
    Null,
}

impl Value {
    pub fn from_wire(value: WireValue) -> anyhow::Result<Self> {
        Ok(match value {
            WireValue::Int(v) => Self::Int(v),
            WireValue::Long(v) => Self::Long(v),
            WireValue::String(v) => Self::String(v.encode_utf16().collect()),
            WireValue::FineCoord(_) => anyhow::bail!("serializable variable has no CS2 stack lane"),
        })
    }
    pub fn into_vm(self) -> anyhow::Result<VmValue> {
        Ok(match self {
            Self::Int(v) => VmValue::Int(v),
            Self::Long(v) => VmValue::Long(v),
            Self::Null => VmValue::Str("null".into()),
            // Strings may hold lone surrogates; the VM lane keeps them
            // as single code units (native910::jstr).
            Self::String(v) => VmValue::Str(native910::jstr::from_units(&v)),
        })
    }
}
impl From<VmValue> for Value {
    fn from(v: VmValue) -> Self {
        match v {
            VmValue::Int(v) => Self::Int(v),
            VmValue::Long(v) => Self::Long(v),
            VmValue::Str(v) => Self::String(native910::jstr::units(&v)),
            VmValue::Null => Self::Null,
        }
    }
}

pub fn default_value(def: &Binding) -> anyhow::Result<Value> {
    if def.data_type.is_none() {
        return Ok(Value::Null);
    }
    Value::from_wire(
        def.default_value()
            .map_err(|e| anyhow::anyhow!("variable default: {e:?}"))?,
    )
}

/// The client variable domain's in-memory mutation state; ui_var_store owns persistence.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientVars {
    pub values: BTreeMap<i32, Value>,
    pub permanent_dirty: bool,
    pub server_dirty: bool,
    pub dirty_ids: BTreeSet<i32>,
}
impl ClientVars {
    pub fn get(&self, def: &Binding) -> anyhow::Result<Value> {
        self.values
            .get(&def.id)
            .cloned()
            .map_or_else(|| default_value(def), Ok)
    }
    pub fn set(&mut self, def: &Binding, mut value: Value) -> anyhow::Result<()> {
        match def.lifetime {
            Some(1) => self.permanent_dirty = true,
            Some(2) => {
                if let Value::String(v) = &mut value {
                    v.truncate(80);
                }
                let old = self.get(def)?;
                // The typed getters can fail before a numeric setter mutates.
                anyhow::ensure!(
                    matches!(
                        (&old, &value),
                        (Value::Int(_), Value::Int(_))
                            | (Value::Long(_), Value::Long(_))
                            | (_, Value::String(_) | Value::Null)
                    ),
                    "client variable getter type mismatch"
                );
                if old != value {
                    self.server_dirty = true;
                    self.dirty_ids.insert(def.id);
                }
            }
            _ => {}
        }
        self.values.insert(def.id, value);
        Ok(())
    }
    /// Reset: permanent and unknown-lifetime values survive.
    /// The local save-dirty flag deliberately survives too.
    pub fn reset(&mut self, definitions: &BTreeMap<i32, Binding>) {
        for (&id, def) in definitions {
            if matches!(def.lifetime, Some(0 | 2)) {
                self.values.remove(&id);
            }
        }
        self.server_dirty = false;
        self.dirty_ids.clear();
    }
}
