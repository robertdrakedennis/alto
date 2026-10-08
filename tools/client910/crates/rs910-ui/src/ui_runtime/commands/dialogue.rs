//! The `resume_*dialog`/`abort_dialog` client packets.
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_resume_countdialog(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let text = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let (opcode, payload) = crate::ui_dialogue::build_resume_count_str(&text);
        self.outgoing.push(opcode);
        self.outgoing.extend(payload);
        Ok(None)
    }

    pub(super) fn cmd_resume_namedialog(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let text = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let (_, payload) =
            crate::ui_dialogue::build_resume_name(&text).ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "dialogue text contains NUL".into(),
            })?;
        self.outgoing
            .push(crate::proto::client::RESUME_P_NAMEDIALOG);
        self.outgoing.extend(payload);
        Ok(None)
    }

    pub(super) fn cmd_resume_stringdialog(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let text = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let (_, payload) =
            crate::ui_dialogue::build_resume_string(&text).ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "dialogue text contains NUL".into(),
            })?;
        self.outgoing
            .push(crate::proto::client::RESUME_P_STRINGDIALOG);
        self.outgoing.extend(payload);
        Ok(None)
    }

    pub(super) fn cmd_resume_objdialog(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let (opcode, payload) = crate::ui_dialogue::build_resume_obj(value);
        self.outgoing.push(opcode);
        self.outgoing.extend(payload);
        Ok(None)
    }

    pub(super) fn cmd_resume_hsldialog(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let value = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let (opcode, payload) = crate::ui_dialogue::build_resume_hsl(value);
        self.outgoing.push(opcode);
        self.outgoing.extend(payload);
        Ok(None)
    }

    pub(super) fn cmd_abort_dialog(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.outgoing.push(crate::proto::client::ABORT_P_DIALOG);
        Ok(None)
    }
}
