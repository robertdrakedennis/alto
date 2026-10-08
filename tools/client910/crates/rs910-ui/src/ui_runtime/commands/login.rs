//! Login and lobby commands (`login_*`, `lobby_*`, `userdetail_lobby_*`).
//!
//! Handlers of the engine-command table ([`super::COMMANDS`]); each body
//! is the former `trap_context` branch for its names, moved verbatim.
use super::super::Engine;
use super::super::LobbyEnterGameRequest;
use super::super::LoginRequest;
use native910::vm::InstructionContext;
use native910::vm::Value;
use native910::vm::VmError;
use native910::vm::VmResult;

impl Engine {
    // resume_*dialog / abort_dialog
    // create the client packet at the VM boundary.  The original commands pop
    // their argument before queuing; keep that ordering and send through
    // the same ordinary outgoing stream as menu and settings actions.
    // requestLogin / enterLobby retain the credential tuple
    // at the VM boundary; the app session owner starts the async worker
    // after this tick.
    pub(super) fn cmd_login_request(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if ints.is_empty() || objs.len() < 3 {
            return Err(VmError::StackUnderflow {
                stack: if ints.is_empty() { "int" } else { "object" },
            });
        }
        let auth_dont_trust = ints.pop().unwrap() == 1;
        let new_auth_preference = objs.pop().unwrap();
        let password = objs.pop().unwrap();
        let username = objs.pop().unwrap();
        // requestLogin / enterLobby
        // return before touching state unless isLoginReady() holds.
        if username.encode_utf16().count() <= 320 && self.login.ready {
            self.login.ready = false;
            self.login.request = Some(LoginRequest {
                username,
                password,
                new_auth_preference,
                auth_dont_trust,
                lobby: c.command == "lobby_enterlobby",
                sso: None,
            });
            self.login.in_progress = true;
            // requestLogin: setReply(-3) on the
            // requested connection; only game logins reset the hop and
            // disallow state (requestState != 132).
            if c.command == "login_request" {
                self.login.reply = -3;
                self.login.hoptime = 0;
                self.login.disallow_result = -1;
                self.login.disallow_trigger = -1;
            } else {
                self.login.lobby_reply = -3;
            }
            self.login.ban_duration = 0;
            self.login.queue_position = -1;
        }
        Ok(None)
    }

    // lobby_enterlobby_sso is the original client's disabled SSO lane: it
    // consumes the credential token and preference flag, then leaves the
    // actual login owner untouched.  Keep its stack effect exact.
    pub(super) fn cmd_lobby_enterlobby_sso(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })?;
        objs.pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        Ok(None)
    }

    pub(super) fn cmd_lobby_entergame(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let auth_dont_trust = ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })? == 1;
        let new_auth_preference = objs
            .pop()
            .ok_or(VmError::StackUnderflow { stack: "object" })?;
        if self.login.lobby_login && !self.login.in_progress {
            self.login.lobby_enter_game = Some(LobbyEnterGameRequest {
                new_auth_preference,
                auth_dont_trust,
            });
            self.login.in_progress = true;
            self.login.reply = -3;
            self.login.hoptime = 0;
            self.login.ban_duration = 0;
            self.login.queue_position = -1;
        }
        Ok(None)
    }

    pub(super) fn cmd_sso_available(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(0)))
    }

    pub(super) fn cmd_sso_displayname(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Str(String::new())))
    }

    pub(super) fn cmd_login_inprogress(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.login.in_progress))))
    }

    pub(super) fn cmd_login_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.reply)))
    }

    pub(super) fn cmd_lobby_enterlobbyreply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.lobby_reply)))
    }

    pub(super) fn cmd_login_hoptime(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.hoptime)))
    }

    pub(super) fn cmd_login_ban_duration(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.ban_duration)))
    }

    pub(super) fn cmd_login_disallowresult(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.disallow_result)))
    }

    pub(super) fn cmd_login_disallowtrigger(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.disallow_trigger)))
    }

    // userflowflags/automatedtestflags expose the retained
    // login metadata in the original client stack order.
    pub(super) fn cmd_userflowflags(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.extend(self.login.user_flow);
        Ok(None)
    }

    pub(super) fn cmd_automatedtestflags(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.extend(self.login.automated_test_flags);
        Ok(None)
    }

    pub(super) fn cmd_login_last_transfer_reply(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        ints.extend([
            self.login.last_transfer_reply,
            self.login.last_transfer_disallow_result,
            self.login.last_transfer_disallow_trigger,
        ]);
        self.login.last_transfer_reply = -2;
        self.login.last_transfer_disallow_result = -1;
        self.login.last_transfer_disallow_trigger = -1;
        Ok(None)
    }

    pub(super) fn cmd_login_resetreply(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // resetLoginState.
        if !self.login.in_progress {
            self.login.request = None;
            self.login.reply = -2;
            self.login.lobby_reply = -2;
            self.login.hoptime = 0;
            self.login.ban_duration = 0;
            self.login.queue_position = -1;
        }
        Ok(None)
    }

    pub(super) fn cmd_login_queue_position(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.queue_position)))
    }

    pub(super) fn cmd_login_cancel(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        // cancelLogin resets
        // both replies only while a login is in progress.
        self.login.request = None;
        self.login.cancel_requested = true;
        if self.login.in_progress {
            self.login.reply = -2;
            self.login.lobby_reply = -2;
        }
        self.login.in_progress = false;
        self.login.hoptime = 0;
        self.login.ban_duration = 0;
        Ok(None)
    }

    pub(super) fn cmd_lobby_leavelobby(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        self.login.logout_requested = true;
        Ok(None)
    }

    // userdetail_lobby_membership.
    pub(super) fn cmd_userdetail_lobby_membership(
        &mut self,
        _c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let now = crate::logic_clock::monotonic_millis();
        ints.push((self.lobby.membership / 60000) as i32);
        ints.push(((self.lobby.membership - now - self.lobby.membership_offset) / 60000) as i32);
        ints.push(i32::from(self.lobby.membership_flag));
        Ok(None)
    }

    // userdetail_lobby_unreadmessages.
    pub(super) fn cmd_userdetail_lobby_unreadmessages(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.unread_messages)))
    }

    // userdetail_lobby_recoveryday/ccexpiry/graceexpiry/
    // dobrequested/membersstats/playage read the corresponding fields
    // from the retained lobby profile.
    pub(super) fn cmd_userdetail_lobby_recoveryday(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.recovery_day)))
    }

    pub(super) fn cmd_userdetail_lobby_ccexpiry(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.cc_expiry)))
    }

    pub(super) fn cmd_userdetail_lobby_graceexpiry(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.grace_expiry)))
    }

    pub(super) fn cmd_userdetail_lobby_dobrequested(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(i32::from(self.lobby.dob_requested))))
    }

    pub(super) fn cmd_userdetail_lobby_membersstats(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.members_stats)))
    }

    pub(super) fn cmd_userdetail_lobby_playage(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.play_age)))
    }

    // userdetail_lobby_loyalty_balance.
    pub(super) fn cmd_userdetail_lobby_loyalty_balance(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.loyalty_balance)))
    }

    // userdetail_lobby_jcoins_balance.
    pub(super) fn cmd_userdetail_lobby_jcoins_balance(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.jcoins_balance)))
    }

    // userdetail_lobby_lastloginday (lobbyLastLoginDay).
    pub(super) fn cmd_userdetail_lobby_lastloginday(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.last_login_day)))
    }

    // lastlogin/userdetail_lobby_lastloginaddress both read
    // the last login address; the retained fallback is the dotted IP.
    pub(super) fn cmd_lastlogin(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        let now = crate::logic_clock::monotonic_millis();
        objs.push(
            self.login
                .last_login
                .as_mut()
                .map(|host| host.text(now))
                .unwrap_or_default(),
        );
        Ok(None)
    }

    // userdetail_lobby_emailstatus (lobbyEmailStatus).
    pub(super) fn cmd_userdetail_lobby_emailstatus(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.lobby.email_status)))
    }

    // map_world.
    pub(super) fn cmd_map_world(
        &mut self,
        _c: &InstructionContext<'_>,
        _ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        Ok(Some(Value::Int(self.login.world)))
    }
}
