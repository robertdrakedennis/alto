//! World-map commands that are not `ClientWorldMap` calls (the
//! disabled 3D view and no-op).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmResult;

impl Engine {
    // executeCommand case 773 -> disabled_command_773
    // (curated name `noopWorldMapCommand`): pops and pushes nothing.
    pub(super) fn cmd_noop_world_map_command(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(None)
    }

    // worldmap_3dview_active returns zero
    // unconditionally in the 910 client (enable/disable themselves throw).
    pub(super) fn cmd_worldmap_3dview_active(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(0)))
    }
}
