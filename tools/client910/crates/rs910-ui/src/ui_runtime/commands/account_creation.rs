//! Account-creation commands (`create_*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::encode_pjstr;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    // The app session owner opens the dedicated CREATE_ACCOUNT_CONNECT
    // socket and installs it before the create_* client packets use the
    // normal lobby writer. Gated on title state 4 with both login owners
    // idle; enters state 12.
    pub(super) fn cmd_create_connectrequest(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if self.login.ready && !self.creation.connect_in_progress {
            self.creation.connect_requested = true;
            self.creation.connect_in_progress = true;
            self.creation.connect_reply = -2;
        }
        Ok(None)
    }

    // Email/name availability requests. `p2(0)`/`p1(0)` right after
    // the opcode IS the size placeholder that `psize2`/`psize1` later
    // backfill with `pos - start`, so the frame is opcode + size + body.
    // The body is the PJSTR plus seven reserved bytes (`pos += 7`) for
    // the disabled tinyenc (ENABLE_TINYENC = false).
    pub(super) fn cmd_create_availablerequest(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let email = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        // The operands are already popped.
        if !self.login.account_creation_connected {
            return Ok(None);
        }
        let value = encode_pjstr(&email).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "email contains NUL".into(),
        })?;
        let body_len = value.len().saturating_add(7);
        let body_len = u16::try_from(body_len).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "email request exceeds psize2".into(),
        })?;
        self.outgoing.extend([
            crate::proto::client::CREATE_CHECK_EMAIL,
            (body_len >> 8) as u8,
            body_len as u8,
        ]);
        self.outgoing.extend(value);
        self.outgoing.extend([0; 7]);
        self.creation.email_reply = -3;
        Ok(None)
    }

    pub(super) fn cmd_create_name_availablerequest(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let name = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        // requestDisplayNameAvailableCheck.
        if !self.login.account_creation_connected {
            return Ok(None);
        }
        let value = encode_pjstr(&name).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "name contains NUL".into(),
        })?;
        let body_len = value.len().saturating_add(7);
        let body_len = u8::try_from(body_len).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "name request exceeds psize1".into(),
        })?;
        self.outgoing
            .extend([crate::proto::client::CREATE_CHECK_NAME, body_len]);
        self.outgoing.extend(value);
        self.outgoing.extend([0; 7]);
        self.creation.name_reply = -3;
        Ok(None)
    }

    pub(super) fn cmd_create_suggest_name_request(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // requestDisplayNameSuggestion.
        if !self.login.account_creation_connected {
            return Ok(None);
        }
        self.outgoing
            .push(crate::proto::client::CREATE_SUGGEST_NAMES);
        self.creation.suggest_reply = -3;
        self.creation.suggested_name = None;
        Ok(None)
    }

    pub(super) fn cmd_create_under13(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.creation.is_under13))))
    }

    pub(super) fn cmd_create_setunder13(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.creation.is_under13 = true;
        Ok(None)
    }

    pub(super) fn cmd_create_step_reached(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let step = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        // requestStatsLogging.
        if !self.login.account_creation_connected {
            return Ok(None);
        }
        // requestStatsLogging queues
        // CREATE_LOG_PROGRESS with one byte of step state on the live
        // lobby connection. The app drains this same outgoing owner.
        self.outgoing
            .extend([crate::proto::client::CREATE_LOG_PROGRESS, step as u8]);
        Ok(None)
    }

    pub(super) fn cmd_create_createrequest(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 || objs.len() < 3 {
            return Err(VmError::StackUnderflow {
                stack: if ints.len() < 2 { "int" } else { "object" },
            });
        }
        let email = objs.pop().unwrap();
        let password = objs.pop().unwrap();
        let username = objs.pop().unwrap();
        let verified = ints.pop().unwrap() == 1;
        let stage = ints.pop().unwrap();
        // requestAccountCreation: nothing, not even the under-13
        // flag, happens outside state 0.
        if !self.login.account_creation_connected {
            return Ok(None);
        }
        if stage < 13 {
            self.creation.is_under13 = true;
        }
        // requestAccountCreation: the
        // `p2(0)` is the psize2 placeholder written below, not body.
        let mut body = Vec::new();
        for (label, value) in [
            ("username", username),
            ("password", password),
            ("email", email),
        ] {
            let encoded = encode_pjstr(&value).ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("{label} contains NUL"),
            })?;
            body.extend(encoded);
            if label == "password" {
                body.push(stage as u8);
                body.push(u8::from(verified));
            }
        }
        body.extend([0; 7]);
        let body_len = u16::try_from(body.len()).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "account request exceeds psize2".into(),
        })?;
        self.outgoing.extend([
            crate::proto::client::CREATE_ACCOUNT,
            (body_len >> 8) as u8,
            body_len as u8,
        ]);
        self.outgoing.extend(body);
        self.creation.account_reply = -3;
        Ok(None)
    }

    // create_email_validate_reply.
    pub(super) fn cmd_create_email_validate_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.creation.email_reply)))
    }

    // create_reply.
    pub(super) fn cmd_create_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.creation.account_reply)))
    }

    // create_name_validate_reply.
    pub(super) fn cmd_create_name_validate_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.creation.name_reply)))
    }

    // create_suggest_name_reply returns the enum id and the
    // suggested string on their respective original stacks.
    pub(super) fn cmd_create_suggest_name_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.push(self.creation.suggest_reply);
        objs.push(self.creation.suggested_name.clone().unwrap_or_default());
        Ok(None)
    }

    pub(super) fn cmd_create_connect_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.creation.connect_reply)))
    }
}
