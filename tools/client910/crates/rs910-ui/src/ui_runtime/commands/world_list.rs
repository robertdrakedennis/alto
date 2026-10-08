//! World-list commands (`worldlist_*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use super::super::WorldSwitchRequest;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_worldlist_fetch(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // The original client allows WORLDLIST_FETCH from lobby state 13 only when the
        // login owner is idle; state 18 is a separate world-list entry
        // path that remains outside this retained login tree.
        let result = self.world_list.request(
            crate::logic_clock::monotonic_millis(),
            (self.login.lobby_login || self.login.world_list_game) && !self.login.in_progress,
            &mut self.outgoing,
        );
        Ok(Some(Value::Int(result)))
    }

    pub(super) fn cmd_worldlist_specific_thisworld(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(
            self.world_list
                .specific(self.login.world)
                .map_or(0, |w| w.flags),
        )))
    }

    pub(super) fn cmd_worldlist_autoworld(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if let Some(target) = &self.login.target_world {
            self.login.world = i32::from(target.world_id);
        }
        Ok(None)
    }

    pub(super) fn cmd_worldlist(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let specific = c.command == "worldlist_specific";
        let world = match c.command {
            "worldlist_start" => self.world_list.start(),
            "worldlist_next" => self.world_list.next(),
            _ => self
                .world_list
                .specific(ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?)
                .cloned(),
        };
        if let Some(w) = world {
            if !specific {
                ints.push(w.id);
            }
            ints.extend([w.flags, w.country, w.players, w.hostpacked]);
            objs.extend([w.activity, w.country_name, w.hostname]);
        } else {
            if !specific {
                ints.push(-1);
                ints.extend([0, 0, 0, 0]);
            } else {
                ints.extend([-1, 0, 0, 0]);
            }
            objs.extend([String::new(), String::new(), String::new()]);
        }
        Ok(None)
    }

    pub(super) fn cmd_worldlist_sort(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 4 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let a = ints.split_off(ints.len() - 4);
        self.world_list.sort(
            a[0],
            a[1] == 1,
            a[2],
            a[3] == 1,
            Some(crate::ui_text_compare::Language::En),
        );
        Ok(None)
    }

    pub(super) fn cmd_worldlist_pingworlds(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // worldlist_pingworlds consumes the
        // argument only in lobby state 13; the retained owner then lets
        // the app's single async resolver advance host-packed values.
        if self.login.lobby_login {
            let enabled = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
            self.world_list.set_resolve_hosts_enabled(enabled == 1);
        }
        Ok(None)
    }

    pub(super) fn cmd_worldlist_switch(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let world_id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let host = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        // `worldlist_switch`
        // only accepts the request while the client is in lobby state 13 and
        // the login owner is idle; setting the world always succeeds.
        let accepted = self.login.lobby_login && !self.login.in_progress;
        if accepted {
            // `setWorld` changes currentWorld immediately;
            // no connection is made until `lobby_entergame` (state 15)
            // logs into that world. The app installs the address.
            self.login.world = world_id;
            self.login.world_switch = Some(WorldSwitchRequest {
                world_id: world_id as u16,
                host,
            });
        }
        Ok(Some(Value::Int(i32::from(accepted))))
    }
}
