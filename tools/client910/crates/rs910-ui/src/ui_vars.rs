//! Borrowed game variable domains.
//! Sparse client values, lifetimes and reset.
//! This owns no copy of the local player's varp array or active actor values.
use crate::{
    entities910::varps::Varps,
    entity_runtime::bits_pack::Inputs,
    protocol910::{
        script_types::script_type,
        varbits::{Binding, Type, ValueError},
        variables::Value as WireValue,
    },
};
use native910::{
    vars::VarScope,
    vm::{Value as VmValue, VarLane},
};
use rs910_core::fault::Fault;
use std::collections::BTreeMap;

// The client variable domain values live in rs910-game.
pub use rs910_game::client_vars::*;

#[derive(Default)]
pub struct State {
    pub persistence: crate::ui_var_store::Persistence,
    pub queries: crate::ui_host::Queries,
    /// `localPlayerGameState.stats`; installed
    /// with the cache, reset at world login.
    pub stats: Option<crate::ui_stats::PlayerStats>,
    /// The quest type list as the quest commands read it.
    pub quests: Option<crate::ui_quests::QuestStore>,
    pub client: ClientVars,
    pub delayed: crate::ui_changes::Changes,
    pub ignored_player_bit_overflows: Vec<i32>,
    pub varc_transmit: Transmit,
    pub string_transmit: Transmit,
    /// Inventory, stat and varclan transmit counters: fed by UPDATE_INV_*, UPDATE_STAT and
    /// VARCLAN packets. Each live packet owner advances its corresponding ring.
    pub inv_transmit: Transmit,
    pub stat_transmit: Transmit,
    pub varclan_transmit: Transmit,
    /// `varClan` sparse domain (CLAN / id 6).
    /// Social packet ownership installs and mutates this map; CS2 reads use
    /// the same typed definition lookup as the other sparse domains.
    pub clan: Option<BTreeMap<i32, Value>>,
    /// `getVarDomain()` (PLAYER_GROUP / id 9).
    /// The normal player-group full/delta packet owner installs this sparse map.
    pub player_group: Option<BTreeMap<i32, Value>>,
    /// `CLAN_SETTING` (id 7): the social owner's shared
    /// `activeClanSettings` domain, linked by the CLANSETTINGS packet owner.
    /// Read-only.
    pub clan_settings: Option<crate::ui_social::ClanSettingsDomain>,
    pub component_changes: std::collections::VecDeque<crate::ui_changes::Change>,
}

pub struct Transmit {
    pub count: i32,
    pub ids: [i32; 64],
}
impl Default for Transmit {
    fn default() -> Self {
        Self {
            count: 0,
            ids: [0; 64],
        }
    }
}
impl Transmit {
    pub fn counter(&self) -> crate::ui_loop::Counter {
        crate::ui_loop::Counter {
            num: self.count,
            ids: self.ids,
        }
    }
    pub fn push(&mut self, id: i32) {
        self.ids[(self.count & 63) as usize] = id;
        self.count = self.count.wrapping_add(1);
    }
}
impl State {
    /// The variable-owner half of `resetTransmitNums`
    /// (called from `logout`): the transmit
    /// counters restart at zero; the original client leaves the id rings' contents in place.
    pub fn reset_transmit_nums(&mut self) {
        for ring in [
            &mut self.inv_transmit,
            &mut self.stat_transmit,
            &mut self.varc_transmit,
            &mut self.string_transmit,
            &mut self.varclan_transmit,
        ] {
            ring.count = 0;
        }
    }
    /// Install the cache-backed player state owners (skill defaults, quest
    /// configs). Missing archives are errors, not empty stubs.
    pub fn with_client(pack: &crate::cache::Pack) -> anyhow::Result<Self> {
        Ok(Self {
            stats: Some(crate::ui_stats::PlayerStats::load(pack)?),
            quests: Some(crate::ui_quests::QuestStore::load(pack)?),
            ..Self::default()
        })
    }
    /// the bit merges into its base varc's type-1 node at enqueue time. A
    /// new node starts from the live client value; a pending node keeps its
    /// queued `int0` (including a client-edit node's unset zero). An
    /// overflow is reported and leaves the node cached but unqueued.
    pub fn set_varc_bit(
        &mut self,
        definitions: &Inputs,
        bit_id: i32,
        value: i32,
    ) -> anyhow::Result<()> {
        let bit = definitions
            .get(bit_id, false)
            .map_err(|e| anyhow::anyhow!("varbit {bit_id}: {e:?}"))?;
        let base = bit
            .binding
            .clone()
            .ok_or_else(|| anyhow::anyhow!("unbound client varbit {bit_id}"))?;
        anyhow::ensure!(
            base.domain == 2,
            "client varbit {bit_id} has non-client base"
        );
        let target = i64::from(base.id);
        let mut current = self.delayed.cache(1, target).ints[0];
        if self.delayed.last_push_new {
            let Value::Int(live) = self.client.get(&base)? else {
                anyhow::bail!("non-integer client varbit base {}", base.id);
            };
            current = live;
            self.delayed.cache(1, target).ints[0] = live;
        }
        match bit.set(current, value) {
            Ok(updated) => {
                self.delayed.cache(1, target).ints[0] = updated;
                self.delayed.push_server(1, target);
                Ok(())
            }
            Err(ValueError::Overflow) => Ok(()),
            Err(e) => anyhow::bail!("varbit {bit_id}: {e:?}"),
        }
    }
    /// updateInterfaces: server writes append transmit IDs
    /// even if unchanged; local edit expiry alone emits nothing.
    pub fn poll(
        &mut self,
        definitions: &Inputs,
        mut now: impl FnMut() -> i64,
    ) -> anyhow::Result<()> {
        while let Some(change) = self.delayed.poll(&mut now) {
            if matches!(change.kind(), 1 | 2) {
                let id = change.target() as i32;
                let def = definitions
                    .binding(2, id)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?
                    .ok_or_else(|| anyhow::anyhow!("client variable definitions absent"))?;
                if change.kind() == 1 {
                    self.client.set(&def, Value::Int(change.ints[0]))?;
                    self.varc_transmit.push(id);
                } else {
                    self.client
                        .set(&def, change.string.map_or(Value::Null, Value::String))?;
                    self.string_transmit.push(id);
                }
            } else {
                self.component_changes.push_back(change);
            }
        }
        Ok(())
    }
}

/// Installed domains for one script execution (executeScript).
/// Uninstalled/secondary domains fail instead of manufacturing zero values.
pub struct Variables<'a> {
    /// loopCycle supplied by the authoritative logic owner for this hook.
    pub cycle: i32,
    pub definitions: &'a Inputs,
    pub state: &'a mut State,
    pub player: Option<&'a mut Varps>,
    pub active_player: Option<&'a mut BTreeMap<i32, WireValue>>,
    pub active_npc: Option<&'a mut BTreeMap<i32, WireValue>>,
    pub now: &'a mut dyn FnMut() -> i64,
    /// varpTransmitNum / varpTransmitted, owned by the entity runtime.
    pub varp_transmit: crate::ui_loop::Counter,
    /// The cam2 scene inputs (`world` base/heightmap, `localPlayerEntity`).
    pub scene: crate::ui_cam2::SceneInput<'a>,
    /// The active renderer, lent for the logic cycle to the commands that
    /// measure the device (`detailget_performance_metric`,
    /// `autosetup_dosetup`); `None` where nothing draws (headless replays).
    pub probe: Option<&'a mut dyn rs910_toolkit::performance_metric::RendererProbe>,
}
impl Variables<'_> {
    fn definition(&self, domain: VarScope, id: u16) -> anyhow::Result<Binding> {
        self.definitions
            .binding(domain as u8, id as i32)
            .map_err(|e| anyhow::anyhow!("variable definition: {e:?}"))?
            .ok_or_else(|| anyhow::anyhow!("uninstalled variable definition domain {domain:?}"))
    }
    pub fn var_type(&self, domain: VarScope, id: u16) -> anyhow::Result<VarLane> {
        let def = self.definition(domain, id)?;
        let base = def.data_type.and_then(script_type).map(|v| v.0);
        match base {
            Some(0) => Ok(VarLane::Int),
            Some(1) => Ok(VarLane::Long),
            Some(2) => Ok(VarLane::String),
            _ => anyhow::bail!("variable {domain:?}:{id} has no CS2 stack type"),
        }
    }
    pub fn integer_default(&self, domain: VarScope, id: u16) -> anyhow::Result<i32> {
        let definition = self.definition(domain, id)?;
        anyhow::ensure!(
            self.var_type(domain, id)? == VarLane::Int,
            "variable has no integer definition"
        );
        let Value::Int(value) = default_value(&definition)? else {
            anyhow::bail!("variable has no integer default")
        };
        Ok(value)
    }
    fn get_binding(&self, def: &Binding, secondary: bool) -> anyhow::Result<Value> {
        match (def.domain, secondary) {
            (0, false) => Ok(Value::Int(
                self.player
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no local player varps"))?
                    .get(def.id)
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?,
            )),
            (2, false) => self.state.client.get(def),
            (6, false) => self
                .state
                .clan
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no clan variable domain"))?
                .get(&def.id)
                .cloned()
                .map_or_else(|| default_value(def), Ok),
            (7, false) => self.clan_setting(def),
            (9, false) => self
                .state
                .player_group
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("no player-group variable domain"))?
                .get(&def.id)
                .cloned()
                .map_or_else(|| default_value(def), Ok),
            (0, true) | (1, false) => {
                let values = if def.domain == 0 {
                    &self.active_player
                } else {
                    &self.active_npc
                };
                let values = values
                    .as_ref()
                    .ok_or_else(|| anyhow::anyhow!("no active entity variable context"))?;
                values
                    .get(&def.id)
                    .cloned()
                    .map_or_else(|| default_value(def), Value::from_wire)
            }
            _ => anyhow::bail!(
                "uninstalled runtime variable domain {} secondary={secondary}",
                def.domain
            ),
        }
    }
    /// The setting keyed
    /// `modegame.game << 16 | var.id`; a missing (or differently typed)
    /// int/long setting reads the var's default, a string reads null.
    fn clan_setting(&self, def: &Binding) -> anyhow::Result<Value> {
        use crate::ui_social::ClanSettingValue as Setting;
        let domain = self
            .state
            .clan_settings
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no clan settings variable domain"))?
            .lock()
            .map_err(|_| anyhow::anyhow!("clan settings domain lock poisoned"))?;
        // Only installed while activeClanSettings != null.
        let settings = domain
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("no clan settings variable domain"))?;
        let key = crate::applet_params::get().mode_game_id() << 16 | def.id;
        let base = def.data_type.and_then(script_type).map(|v| v.0);
        match (base, settings.get(&key)) {
            (Some(0), Some(Setting::Int(v))) => Ok(Value::Int(*v)),
            (Some(1), Some(Setting::Long(v))) => Ok(Value::Long(*v)),
            (Some(0 | 1), _) => default_value(def),
            (Some(2), Some(Setting::String(v))) => Ok(Value::String(v.encode_utf16().collect())),
            (Some(2), _) => Ok(Value::Null),
            _ => anyhow::bail!(
                "{}",
                Fault::InvalidState
                    .message(format_args!("clan setting {} is not a string", def.id))
            ),
        }
    }
    pub fn get(&self, domain: VarScope, id: u16, secondary: bool) -> anyhow::Result<VmValue> {
        let value = self
            .get_binding(&self.definition(domain, id)?, secondary)?
            .into_vm()?;
        anyhow::ensure!(
            matches!(
                (self.var_type(domain, id)?, &value),
                (VarLane::Int, VmValue::Int(_))
                    | (VarLane::Long, VmValue::Long(_))
                    | (VarLane::String, VmValue::Str(_))
            ),
            "variable getter type mismatch"
        );
        Ok(value)
    }
    fn set_binding(&mut self, def: &Binding, secondary: bool, value: Value) -> anyhow::Result<()> {
        match (def.domain, secondary) {
            (0, false) => {
                let Value::Int(v) = value else {
                    anyhow::bail!("local player varps are integer arrays")
                };
                self.player
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("no local player varps"))?
                    .set_local(def.id, v, (self.now)())
                    .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            }
            (2, false) => self.state.client.set(def, value)?,
            // Every setter throws.
            (7, false) => {
                anyhow::bail!(
                    "{}",
                    Fault::InvalidState.message("the clan settings domain is read-only")
                )
            }
            (0, true) | (1, false) => {
                let values = if def.domain == 0 {
                    &mut self.active_player
                } else {
                    &mut self.active_npc
                };
                let values = values
                    .as_mut()
                    .ok_or_else(|| anyhow::anyhow!("no active entity variable context"))?;
                let value = match value {
                    Value::Int(v) => WireValue::Int(v),
                    Value::Long(v) => WireValue::Long(v),
                    Value::String(v) => WireValue::String(String::from_utf16(&v)?),
                    Value::Null => {
                        anyhow::bail!("null sparse actor value has no wire representation")
                    }
                };
                values.insert(def.id, value);
            }
            _ => anyhow::bail!(
                "uninstalled runtime variable domain {} secondary={secondary}",
                def.domain
            ),
        }
        Ok(())
    }
    pub fn set(
        &mut self,
        domain: VarScope,
        id: u16,
        secondary: bool,
        value: VmValue,
    ) -> anyhow::Result<()> {
        let def = self.definition(domain, id)?;
        if domain == VarScope::Client {
            let kind = match self.var_type(domain, id)? {
                VarLane::Int => Some(1),
                VarLane::String => Some(2),
                VarLane::Long => None,
            };
            if let Some(kind) = kind {
                self.state
                    .delayed
                    .push_client(kind, id as i64, (self.now)());
            }
        }
        self.set_binding(&def, secondary, value.into())
    }
    fn bit(&self, id: u16) -> anyhow::Result<Type> {
        self.definitions
            .get(id as i32, false)
            .map_err(|e| anyhow::anyhow!("varbit {id}: {e:?}"))
    }
    /// Domain of a varbit's base variable (`baseVar.domain`).
    pub fn bit_domain(&self, id: u16) -> anyhow::Result<u8> {
        let bit = self.bit(id)?;
        Ok(bit
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("unbound varbit {id}"))?
            .domain)
    }
    /// Generic base lookup for the shared checkbox getter (CS2 2526).
    /// Returns (base varp id, start bit, end bit) with
    /// no per-setting branches, so new Inventory rows are data, not new code.
    #[cfg_attr(
        not(test),
        allow(dead_code, reason = "CS2 2526 helper; exercised by tests only")
    )]
    pub fn bit_base(&self, id: u16) -> anyhow::Result<(i32, i32, i32)> {
        let bit = self.bit(id)?;
        Ok((bit.base_id, bit.start, bit.end))
    }
    pub fn get_bit(&self, id: u16, secondary: bool) -> anyhow::Result<i32> {
        let bit = self.bit(id)?;
        let base = bit
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("unbound varbit {id}"))?;
        let Value::Int(old) = self.get_binding(base, secondary)? else {
            anyhow::bail!("non-integer varbit base")
        };
        bit.get(old)
            .map_err(|e| anyhow::anyhow!("varbit {id}: {e:?}"))
    }
    pub fn set_bit(&mut self, id: u16, secondary: bool, value: i32) -> anyhow::Result<()> {
        let bit = self.bit(id)?;
        let base = bit
            .binding
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("unbound varbit {id}"))?;
        let Value::Int(old) = self.get_binding(base, secondary)? else {
            anyhow::bail!("non-integer varbit base")
        };
        let updated = match bit.set(old, value) {
            Ok(v) => v,
            Err(ValueError::Overflow) if base.domain == 0 && !secondary => {
                self.state.ignored_player_bit_overflows.push(id as i32);
                return Ok(());
            }
            Err(e) => anyhow::bail!("varbit {id}: {e:?}"),
        };
        // pop_varbit does not call onVarC; only the domain setter runs here.
        self.set_binding(base, secondary, Value::Int(updated))
    }
}

/// Variable operations are authoritative even while the surrounding UI engine
/// dispatcher is being replaced. Arrays and engine effects retain their host.
#[cfg(any(test, feature = "test-hooks"))] // test-only VM host
pub struct WithVariables<'a, H> {
    pub engine: &'a mut H,
    pub variables: Variables<'a>,
}
#[cfg(any(test, feature = "test-hooks"))] // test-only VM host helper
fn vm_error(e: anyhow::Error) -> native910::vm::VmError {
    native910::vm::VmError::UnknownCommand {
        command: format!("runtime variable: {e:#}"),
    }
}
#[cfg(any(test, feature = "test-hooks"))] // test-only VM host
impl<H: native910::vm::Host> native910::vm::Host for WithVariables<'_, H> {
    fn trap_resource_context(
        &mut self,
        operation: native910::execution::HostOperation,
        context: &native910::vm::InstructionContext<'_>,
        resource: Option<&native910::execution::Resource>,
        ints: &mut Vec<i32>,
        objects: &mut Vec<Option<String>>,
        longs: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<VmValue>> {
        self.engine
            .trap_resource_context(operation, context, resource, ints, objects, longs)
    }
    fn var_type(&mut self, d: VarScope, id: u16) -> native910::vm::VmResult<VarLane> {
        self.variables.var_type(d, id).map_err(vm_error)
    }
    fn var_integer_default(&mut self, d: VarScope, id: u16) -> native910::vm::VmResult<i32> {
        self.variables.integer_default(d, id).map_err(vm_error)
    }
    fn var_get(&mut self, d: VarScope, id: u16, s: bool) -> native910::vm::VmResult<VmValue> {
        self.variables.get(d, id, s).map_err(vm_error)
    }
    fn var_set(
        &mut self,
        d: VarScope,
        id: u16,
        s: bool,
        v: VmValue,
    ) -> native910::vm::VmResult<()> {
        self.variables.set(d, id, s, v).map_err(vm_error)
    }
    fn varbit_get(&mut self, id: u16, s: bool) -> native910::vm::VmResult<i32> {
        self.variables.get_bit(id, s).map_err(vm_error)
    }
    fn varbit_set(&mut self, id: u16, s: bool, v: i32) -> native910::vm::VmResult<()> {
        self.variables.set_bit(id, s, v).map_err(vm_error)
    }
    fn array_define(&mut self, id: i32, n: usize) -> native910::vm::VmResult<()> {
        self.engine.array_define(id, n)
    }
    fn array_len(&mut self, id: i32) -> native910::vm::VmResult<usize> {
        self.engine.array_len(id)
    }
    fn array_get(&mut self, id: i32, n: i32) -> native910::vm::VmResult<i32> {
        self.engine.array_get(id, n)
    }
    fn array_set(&mut self, id: i32, n: i32, v: i32) -> native910::vm::VmResult<()> {
        self.engine.array_set(id, n, v)
    }
    fn trap_context(
        &mut self,
        c: &native910::vm::InstructionContext<'_>,
        i: &mut Vec<i32>,
        s: &mut Vec<String>,
        l: &mut Vec<i64>,
    ) -> native910::vm::VmResult<Option<VmValue>> {
        if let Some(result) =
            self.variables
                .state
                .queries
                .dispatch(c.command, self.variables.cycle, i, s, None)
        {
            return result;
        }
        if let Some(result) = crate::ui_player_state::dispatch(
            &self.variables,
            self.variables.state.stats.as_ref(),
            self.variables.state.quests.as_ref(),
            c.command,
            i,
            s,
        ) {
            return result;
        }
        self.engine.trap_context(c, i, s, l)
    }
    fn take_effects(&mut self) -> Vec<native910::vm::HostEffect> {
        self.engine.take_effects()
    }
}

#[cfg(test)]
mod set_varc_bit_tests {
    use super::*;

    /// varc 0 (int) and varbit 3 = client var 0 bits 1..8.
    fn inputs() -> Inputs {
        let mut inputs = Inputs {
            definitions: BTreeMap::new(),
            raw: BTreeMap::new(),
            count: 4,
        };
        inputs.definitions.insert(
            2,
            [(
                0,
                Binding {
                    domain: 2,
                    id: 0,
                    data_type: Some(0),
                    lifetime: Some(0),
                    legacy: true,
                    client_code: 0,
                },
            )]
            .into_iter()
            .collect(),
        );
        inputs.raw.insert(3, vec![1, 2, 0, 0, 2, 1, 8, 0]);
        inputs
    }

    /// SETVARC then SETVARCBIT on the same
    /// base coalesce into one queued type-1 node; the bit applies to the
    /// pending value, and the base id is transmitted once.
    #[test]
    fn varc_bit_merges_into_pending_base_node() {
        let inputs = inputs();
        let mut state = State::default();
        let def = inputs.binding(2, 0).unwrap().unwrap();
        state.client.set(&def, Value::Int(0x200)).unwrap();
        state.delayed.push_server(1, 0).ints[0] = 0x401;
        state.set_varc_bit(&inputs, 3, 5).unwrap();
        assert_eq!(state.delayed.server.len(), 1);
        state.poll(&inputs, || 0).unwrap();
        assert_eq!(
            state.client.get(&def).unwrap(),
            Value::Int(0x401 | (5 << 1))
        );
        assert_eq!(state.varc_transmit.count, 1);
        // A fresh node starts from the live value.
        state.set_varc_bit(&inputs, 3, 1).unwrap();
        state.poll(&inputs, || 0).unwrap();
        assert_eq!(
            state.client.get(&def).unwrap(),
            Value::Int(0x401 | (1 << 1))
        );
        // Overflow leaves the value and queue untouched.
        state.set_varc_bit(&inputs, 3, 1 << 9).unwrap();
        assert!(state.delayed.server.is_empty());
    }
}

#[cfg(test)]
mod string_tests {
    use super::*;

    #[test]
    fn lone_surrogates_cross_the_vm_boundary_unchanged() {
        // A String may hold an unpaired surrogate; the VM lane keeps it
        // as one code unit (native910::jstr) instead of failing the read.
        let value = Value::String(vec![0x61, 0xD83D, 0x62]);
        let vm = value.clone().into_vm().unwrap();
        let VmValue::Str(text) = &vm else {
            panic!("string lane");
        };
        assert_eq!(native910::jstr::len(text), 3);
        assert_eq!(Value::from(vm), value);
    }
}
