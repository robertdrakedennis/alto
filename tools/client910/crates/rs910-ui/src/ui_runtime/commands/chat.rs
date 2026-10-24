//! `ChatHistory` and chat-filter commands (`chat_*`, `mes`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::absent;
use super::super::encode_pjstr;
use super::super::strip_chat_prefix;
use super::super::truncate_utf16;
use super::super::Engine;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    pub(super) fn cmd_chat_setmode(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let mode = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        self.outgoing
            .extend([crate::proto::client::CHAT_SETMODE, mode as u8]);
        Ok(None)
    }

    pub(super) fn cmd_chat_sendabusereport(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.len() < 2 || objs.len() < 2 {
            return Err(VmError::StackUnderflow {
                stack: if ints.len() < 2 { "int" } else { "object" },
            });
        }
        let args = ints.split_off(ints.len() - 2);
        let reason = truncate_utf16(objs.pop().unwrap(), 80);
        let target = objs.pop().unwrap();
        let target = encode_pjstr(&target).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "abuse-report target contains NUL".into(),
        })?;
        let reason = encode_pjstr(&reason).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "abuse-report reason contains NUL".into(),
        })?;
        // SEND_SNAPSHOT (11/-1) with a
        // `p1(pjstrlen(target) + 2 + pjstrlen(reason))` length byte.
        let body_len = target.len().wrapping_add(reason.len()).wrapping_add(2);
        self.outgoing
            .extend([crate::proto::client::SEND_SNAPSHOT, body_len as u8]);
        self.outgoing.extend(target);
        self.outgoing.push((args[0] as u8).wrapping_sub(1));
        self.outgoing.push(args[1] as u8);
        self.outgoing.extend(reason);
        Ok(None)
    }

    pub(super) fn cmd_chat_sendpublic(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let message = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        if self.account.staff_mod_level == 0
            && (self.account.dob_verified && !self.account.player_is_quickchat
                || self.account.logged_in_quickchat)
        {
            return Ok(None);
        }
        use rs910_core::texts::Msg;
        // No colour prefix is colour 0; the effect prefix follows the colour
        // (wave, wave2, shake, scroll, slide are effects 1..5, none is 0).
        let (colour, message) = strip_chat_prefix(
            &message,
            &[
                Msg::Chatcol0,
                Msg::Chatcol1,
                Msg::Chatcol2,
                Msg::Chatcol3,
                Msg::Chatcol4,
                Msg::Chatcol5,
                Msg::Chatcol6,
                Msg::Chatcol7,
                Msg::Chatcol8,
                Msg::Chatcol9,
                Msg::Chatcol10,
                Msg::Chatcol11,
            ],
        )
        .map_or((0, message), |(index, rest)| (index as i32, rest));
        let (effect, message) = strip_chat_prefix(
            &message,
            &[
                Msg::Chateffect1,
                Msg::Chateffect2,
                Msg::Chateffect3,
                Msg::Chateffect4,
                Msg::Chateffect5,
            ],
        )
        .map_or((0, message), |(index, rest)| (index as i32 + 1, rest));
        let wordpack = self
            .configs
            .wordpack
            .as_ref()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "chat Huffman coder not loaded".into(),
            })?;
        let compressed = wordpack
            .encode_string(&message)
            .map_err(|error| VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("chat text encode: {error:#}"),
            })?;
        let body_len = compressed.len().saturating_add(2);
        if body_len > u8::MAX as usize {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "public chat payload exceeds psize1".into(),
            });
        }
        self.outgoing.extend([
            crate::proto::client::MESSAGE_PUBLIC,
            body_len as u8,
            colour as u8,
            effect as u8,
        ]);
        self.outgoing.extend(compressed);
        Ok(None)
    }

    pub(super) fn cmd_chat_sendprivate(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let message = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        let recipient = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        if self.account.staff_mod_level == 0
            && (self.account.dob_verified && !self.account.player_is_quickchat
                || self.account.logged_in_quickchat)
        {
            return Ok(None);
        }
        let recipient = encode_pjstr(&recipient).ok_or_else(|| VmError::TrapFailed {
            command: c.command.into(),
            reason: "private-chat recipient contains NUL".into(),
        })?;
        let wordpack = self
            .configs
            .wordpack
            .as_ref()
            .ok_or_else(|| VmError::TrapFailed {
                command: c.command.into(),
                reason: "chat Huffman coder not loaded".into(),
            })?;
        let compressed = wordpack
            .encode_string(&message)
            .map_err(|error| VmError::TrapFailed {
                command: c.command.into(),
                reason: format!("chat text encode: {error:#}"),
            })?;
        let body_len = recipient.len().saturating_add(compressed.len());
        if body_len > u16::MAX as usize {
            return Err(VmError::TrapFailed {
                command: c.command.into(),
                reason: "private chat payload exceeds psize2".into(),
            });
        }
        self.outgoing.extend([
            crate::proto::client::MESSAGE_PRIVATE,
            (body_len >> 8) as u8,
            body_len as u8,
        ]);
        self.outgoing.extend(recipient);
        self.outgoing.extend(compressed);
        Ok(None)
    }

    pub(super) fn cmd_chat_getfilter(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let filter = match c.command {
            "chat_getfilter_public" => Some(0),
            "chat_getfilter_private" => Some(1),
            "chat_getfilter_trade" => Some(2),
            _ => None,
        };

        if let Some(index) = filter {
            return Ok(Some(Value::Int(self.messages.filters[index].unwrap_or(-1))));
        }
        // Unreachable: the table routes only the names above here.
        Err(absent(c.command))
    }

    pub(super) fn cmd_chat_setfilter(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // The existing server frames opcode 88 and currently logs it as
        // unhandled; emitting the original packet does not invent server state.
        if ints.len() < 3 {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let a = ints.split_off(ints.len() - 3);
        let private = if (0..=2).contains(&a[1]) { a[1] } else { 1 };
        self.messages.filters = [Some(a[0]), Some(private), Some(a[2])];
        self.outgoing
            .extend([88, a[0] as u8, private as u8, a[2] as u8]);
        Ok(None)
    }

    // Faithful fresh-login empty states (initial values, no server updates
    // yet). Social presence queries are
    // deliberately left to the retained social owner below; intercepting
    // them here would hide live clan/player-group state behind a zero.
    // Config-backed (inv_size/nc_param/quest) and world/display/audio
    // services remain explicit dependencies, not zero stubs.
    // chat_playername/chat_playername_unfiltered
    // the packet owner refreshes this
    // retained name from the live local-player appearance before each
    // event. Fresh login remains the original empty string.
    pub(super) fn cmd_chat_playername(
        &mut self,
        c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // chat_playername is
        // getNameWithExtras(true) (title substituted); the unfiltered
        // variant is getName(false).
        let name = self.social.local_player_name.clone();
        if c.command == "chat_playername" {
            if let Some(title) = self.social.local_player_title.as_ref() {
                return Ok(Some(Value::Str(title.replace("<name>", &name))));
            }
        }
        Ok(Some(Value::Str(name)))
    }

    // get_col_tag; colTag uses
    // Integer.toHexString (unsigned, lowercase, unpadded).
    pub(super) fn cmd_get_col_tag(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let colour = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        Ok(Some(Value::Str(format!("<col={:x}>", colour as u32))))
    }

    pub(super) fn cmd_mes(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let message = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        self.messages.history.mes(message);
        self.messages.changed = true;
        Ok(None)
    }

    pub(super) fn cmd_chat_lastuid(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.messages.history.last_uid())))
    }

    pub(super) fn cmd_chat_clear(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.messages.history.clear();
        Ok(None)
    }

    pub(super) fn cmd_chat_get(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let id = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        let value = match c.command {
            "chat_gethistorylength" => self.messages.history.length(id) as i32,
            "chat_getnextuid" => self.messages.history.next_uid_of(id),
            _ => self.messages.history.previous_uid(id),
        };
        Ok(Some(Value::Int(value)))
    }

    pub(super) fn cmd_chat_gethistory_by(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let typed = c.command == "chat_gethistory_bytypeandline";
        if ints.len() < if typed { 2 } else { 1 } {
            return Err(VmError::StackUnderflow { stack: "int" });
        }
        let id = ints.pop().unwrap();
        let row = if typed {
            let kind = ints.pop().unwrap();
            if !self.messages.history.has_type(kind) {
                return Err(VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "chat history type not initialized".into(),
                });
            }
            self.messages.history.get_by_type_and_line(kind, id)
        } else {
            self.messages.history.get_by_uid(id)
        };
        if let Some(row) = row {
            let time =
                crate::ui_time::format_time_local(row.time).ok_or_else(|| VmError::TrapFailed {
                    command: c.command.into(),
                    reason: "local calendar timestamp out of range".into(),
                })?;
            ints.extend([
                if typed { row.uid } else { row.chat_type },
                row.flags,
                row.phrase,
                row.crown.unwrap_or(-1),
            ]);
            objs.extend([
                time,
                row.name.clone(),
                row.name_unfiltered.clone(),
                row.name_simple.clone(),
                row.clan.clone().unwrap_or_default(),
                row.message.clone(),
            ]);
        } else {
            ints.extend([-1, 0, 0, -1]);
            objs.extend(std::iter::repeat_n(String::new(), 6));
        }
        Ok(None)
    }
}
