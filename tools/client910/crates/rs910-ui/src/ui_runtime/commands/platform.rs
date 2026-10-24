//! Window, input and platform commands (fullscreen, mouse, held keys,
//! focus, marketing/notification no-ops, reboot timer, telemetry count).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    // marketing_init/notifications_init and their settings
    // opener are intentional original no-ops in 910. `marketing_sendevent`
    // only consumes its event object; letting these fall through as
    // UnknownCommand aborts otherwise valid lobby/UI scripts.
    pub(super) fn cmd_marketing_init(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(None)
    }

    pub(super) fn cmd_marketing_sendevent(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        objs.pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        Ok(None)
    }

    // keyheld_alt/ctrl/shift.
    pub(super) fn cmd_keyheld(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if let Some(key) = match c.command {
            "keyheld_alt" => Some(86),
            "keyheld_ctrl" => Some(82),
            "keyheld_shift" => Some(81),
            _ => None,
        } {
            return Ok(Some(Value::Int(i32::from(self.platform.held_keys[key]))));
        }
        Err(absent(c.command))
    }

    // get_mousebuttons: left, middle, right held.
    pub(super) fn cmd_get_mousebuttons(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.extend(self.platform.mouse_buttons.iter().map(|&b| i32::from(b)));
        Ok(None)
    }

    // reboottimer.
    pub(super) fn cmd_reboottimer(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.reboot_timer)))
    }

    pub(super) fn cmd_detailget_chosesafemode(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.platform.chose_safe_mode))))
    }

    pub(super) fn cmd_detailget_safemode(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.platform.safe_mode))))
    }

    pub(super) fn cmd_fullscreen_lastmode(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let index = if self.platform.fullscreen_allowed {
            let modes =
                self.platform
                    .fullscreen_modes
                    .as_ref()
                    .ok_or_else(|| VmError::TrapFailed {
                        command: c.command.into(),
                        reason: "display modes not installed by window owner".into(),
                    })?;
            modes
                .iter()
                .position(|m| [m.width, m.height] == self.platform.last_fullscreen_size)
                .map_or(-1, |i| i as i32)
        } else {
            -1
        };
        Ok(Some(Value::Int(index)))
    }

    pub(super) fn cmd_fullscreen_getmode(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let index = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        if self.platform.fullscreen_allowed {
            let modes =
                self.platform
                    .fullscreen_modes
                    .as_ref()
                    .ok_or_else(|| VmError::TrapFailed {
                        command: c.command.into(),
                        reason: "display modes not installed by window owner".into(),
                    })?;
            // No mode inside the size limit: answered like a platform
            // without fullscreen (see `WindowState::dispatch`).
            if modes.is_empty() {
                ints.extend([0, 0]);
                return Ok(None);
            }
            let mode = usize::try_from(index)
                .ok()
                .and_then(|i| modes.get(i))
                .ok_or_else(|| VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "fullscreen mode index outside array".into(),
                })?;
            ints.extend([mode.width, mode.height]);
        } else {
            ints.extend([0, 0]);
        }
        Ok(None)
    }

    // fullscreen_modecount.
    pub(super) fn cmd_fullscreen_modecount(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if !self.platform.fullscreen_allowed {
            return Ok(Some(Value::Int(0)));
        }
        let modes = self
            .platform
            .fullscreen_modes
            .as_ref()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "display modes not installed by the window owner".into(),
            })?;
        Ok(Some(Value::Int(modes.len() as i32)))
    }

    pub(super) fn cmd_getwindowmode(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.platform.window_mode)))
    }

    // Reads the frame snapshot, not raw events.
    pub(super) fn cmd_get_mousex(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.platform.mouse[0])))
    }

    pub(super) fn cmd_get_mousey(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.platform.mouse[1])))
    }

    // telemetry_get_group_count → TelemetryGrid over
    // the packet-fed telemetry (host_builtins).
    pub(super) fn cmd_telemetry_get_group_count(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.builtins.telemetry.group_count())))
    }

    pub(super) fn cmd_applet_hasfocus(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.platform.app_focused))))
    }
}
