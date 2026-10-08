//! Quick-chat phrase and category commands (`chatphrase_*`, `chatcat_*`,
//! `activechatphrase_*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::encode_pjstr;
use super::super::ActiveChatPhrase;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

/// A phrase or category list read past its end fails, as it does in the original.
fn range_error(command: &str) -> VmError {
    VmError::TrapFailed {
        command: command.into(),
        reason: "index outside the phrase or category list".into(),
    }
}

impl Engine {
    pub(super) fn cmd_chatphrase_getdynamiccommandcount(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
        let count = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.dynamic_count(id))
            .unwrap_or(0);
        ints.push(count as i32);
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_getdynamiccommand(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let index = usize::try_from(ints.pop().unwrap()).unwrap_or(usize::MAX);
        let id = ints.pop().unwrap() as u16;
        let command = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.dynamic_command(id, index))
            .ok_or_else(|| range_error(c.command))?;
        ints.push(i32::from(command));
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_gettext(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
        objs.push(
            self.configs
                .quickchat
                .as_ref()
                .and_then(|store| store.text_display(id))
                .unwrap_or_default(),
        );
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_find(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let query = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let global = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? == 1;
        self.messages.phrase_find_results = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.find_phrases(&query, global));
        self.messages.phrase_find_index = 0;
        ints.push(
            self.messages
                .phrase_find_results
                .as_ref()
                .map_or(-1, |results| results.len() as i32),
        );
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_findnext(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let result = self
            .messages
            .phrase_find_results
            .as_ref()
            .and_then(|results| results.get(self.messages.phrase_find_index).copied())
            .map_or(-1, i32::from);
        if result != -1 {
            self.messages.phrase_find_index += 1;
        }
        ints.push(result);
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_findrestart(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.messages.phrase_find_index = 0;
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_getdynamiccommandparam_enum(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 3 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let args = ints.split_off(ints.len() - 3);
        let id = args[0] as u16;
        let dynamic = usize::try_from(args[1]).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "negative quick-chat dynamic index".into(),
        })?;
        let parameter = usize::try_from(args[2]).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "negative quick-chat parameter index".into(),
        })?;
        let (command, value) = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.dynamic_parameter(id, dynamic, parameter))
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "quick-chat enum parameter is outside the phrase definition".into(),
            })?;
        if command != 0 {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("quick-chat dynamic command {command} is not ENUM_STRING"),
            });
        }
        ints.push(value);
        Ok(None)
    }

    pub(super) fn cmd_chatcat_getdesc(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
        objs.push(
            self.configs
                .quickchat
                .as_ref()
                .and_then(|store| store.category_description(id))
                .unwrap_or_default()
                .to_owned(),
        );
        Ok(None)
    }

    pub(super) fn cmd_chatcat_get(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
        let count = self.configs.quickchat.as_ref().map_or(0, |store| {
            if c.command == "chatcat_getsubcatcount" {
                store.category_sub_count(id)
            } else {
                store.category_phrase_count(id)
            }
        });
        ints.push(count as i32);
        Ok(None)
    }

    pub(super) fn cmd_chatcat_getsubcat(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let index = usize::try_from(ints.pop().unwrap()).unwrap_or(usize::MAX);
        let id = ints.pop().unwrap() as u16;
        let result = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| {
                if c.command == "chatcat_getsubcat" {
                    store.category_sub(id, index)
                } else {
                    store.category_phrase(id, index)
                }
            })
            .ok_or_else(|| range_error(c.command))?;
        ints.push(i32::from(result));
        Ok(None)
    }

    pub(super) fn cmd_chatcat_getsubcatshortcut(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let index = usize::try_from(ints.pop().unwrap()).unwrap_or(usize::MAX);
        let id = ints.pop().unwrap() as u16;
        let result = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| {
                if c.command == "chatcat_getsubcatshortcut" {
                    store.category_sub_shortcut(id, index)
                } else {
                    store.category_phrase_shortcut(id, index)
                }
            })
            .ok_or_else(|| range_error(c.command))?;
        ints.push(result as i32);
        Ok(None)
    }

    pub(super) fn cmd_chatcat_find(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let shortcut = ints.pop().unwrap();
        let id = ints.pop().unwrap() as u16;
        let result = if shortcut == -1 {
            -1
        } else {
            let shortcut = char::from_u32(shortcut as u32 & 0xffff).unwrap_or('\0');
            self.configs.quickchat.as_ref().map_or(-1, |store| {
                if c.command == "chatcat_findsubcatbyshortcut" {
                    store.category_find_sub_by_shortcut(id, shortcut)
                } else {
                    store.category_find_phrase_by_shortcut(id, shortcut)
                }
            })
        };
        ints.push(result);
        Ok(None)
    }

    pub(super) fn cmd_chatphrase_getautoresponse(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if c.command == "chatphrase_getautoresponsecount" {
            let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
            ints.push(
                self.configs
                    .quickchat
                    .as_ref()
                    .map_or(0, |store| store.auto_response_count(id)) as i32,
            );
        } else {
            if ints.len() < 2 {
                return Err(VmError::StackUnderflow { stack: "int" });
            }
            let index = usize::try_from(ints.pop().unwrap()).unwrap_or(usize::MAX);
            let id = ints.pop().unwrap() as u16;
            let response = self
                .configs
                .quickchat
                .as_ref()
                .and_then(|store| store.auto_response(id, index))
                .ok_or_else(|| range_error(c.command))?;
            ints.push(i32::from(response));
        }
        Ok(None)
    }

    pub(super) fn cmd_activechatphrase_prepare(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? as u16;
        let count = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.dynamic_count(id))
            .unwrap_or(0);
        self.messages.active_chat_phrase = Some(ActiveChatPhrase {
            id,
            dynamics: vec![0; count],
        });
        Ok(None)
    }

    pub(super) fn cmd_activechatphrase_setdynamic(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let value = ints.pop().unwrap();
        let index = usize::try_from(ints.pop().unwrap()).map_err(|_| VmError::TrapFailed {
            command: c.command.into(),
            reason: "negative quick-chat dynamic index".into(),
        })?;
        let phrase =
            self.messages
                .active_chat_phrase
                .as_mut()
                .ok_or_else(|| VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "active quick-chat phrase is null".into(),
                })?;
        let slot = phrase
            .dynamics
            .get_mut(index)
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "quick-chat dynamic index outside phrase".into(),
            })?;
        *slot = value;
        Ok(None)
    }

    pub(super) fn cmd_activechatphrase_send(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let mode = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let phrase =
            self.messages
                .active_chat_phrase
                .clone()
                .ok_or_else(|| VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "active quick-chat phrase is null".into(),
                })?;
        let dynamic = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.transmit_values(phrase.id, &phrase.dynamics))
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "quick-chat phrase definition missing".into(),
            })?;
        let body_len = 3usize.saturating_add(dynamic.len());
        if body_len > u8::MAX as usize {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "public quick-chat payload exceeds psize1".into(),
            });
        }
        self.outgoing.extend([
            crate::proto::client::MESSAGE_QUICKCHAT_PUBLIC,
            body_len as u8,
            mode as u8,
            (phrase.id >> 8) as u8,
            phrase.id as u8,
        ]);
        self.outgoing.extend(dynamic);
        Ok(None)
    }

    pub(super) fn cmd_activechatphrase_sendprivate(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let recipient = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let recipient = encode_pjstr(&recipient).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "private quick-chat recipient contains NUL".into(),
        })?;
        let phrase =
            self.messages
                .active_chat_phrase
                .clone()
                .ok_or_else(|| VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "active quick-chat phrase is null".into(),
                })?;
        let dynamic = self
            .configs
            .quickchat
            .as_ref()
            .and_then(|store| store.transmit_values(phrase.id, &phrase.dynamics))
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "quick-chat phrase definition missing".into(),
            })?;
        let body_len = recipient
            .len()
            .saturating_add(2)
            .saturating_add(dynamic.len());
        if body_len > u8::MAX as usize {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "private quick-chat payload exceeds psize1".into(),
            });
        }
        self.outgoing.extend([
            crate::proto::client::MESSAGE_QUICKCHAT_PRIVATE,
            body_len as u8,
        ]);
        self.outgoing.extend(recipient);
        self.outgoing
            .extend([(phrase.id >> 8) as u8, phrase.id as u8]);
        self.outgoing.extend(dynamic);
        Ok(None)
    }
}
