//! `executeCommand` for the retained [`Engine`]: a static
//! command table (name -> handler, [`COMMANDS`]) over the per-family handler
//! modules, then the owner dispatchers ([`OWNERS`]) that match their own
//! names (social, config queries, stockmarket, world map, cam2, builtins,
//! host_game), then the unsupported-command fallback.
//!
//! The table replaces the former `trap_context` string-compare chain
//! (code-quality programme Phase 4.2). Every table name was one diverging
//! branch of that chain, and no owner claims a table name, so
//! looking the name up first reaches the same handler as the old order.
use super::{absent, host_game, ActiveEntity, Engine};
use native910::vm::{InstructionContext, Value, VmResult};

mod account;
mod account_creation;
mod chat;
mod config;
mod coord;
mod dialogue;
mod entity;
mod inventory;
mod login;
mod minimenu;
mod platform;
mod quickchat;
mod social;
mod sound;
mod stockmarket;
mod text;
mod world_list;
mod world_map;

#[cfg(test)]
mod tests;

/// One engine command: `trap_context`'s arguments.
type Handler = fn(
    &mut Engine,
    &InstructionContext<'_>,
    &mut Vec<i32>,
    &mut Vec<String>,
    &mut Vec<i64>,
) -> VmResult<Option<Value>>;

/// An owner dispatcher: `None` when the command is not one of its names.
type Owner = fn(
    &mut Engine,
    &InstructionContext<'_>,
    &mut Vec<i32>,
    &mut Vec<String>,
    &mut Vec<i64>,
) -> Option<VmResult<Option<Value>>>;

/// The engine commands with a handler here, sorted by name (byte order) for
/// the binary search in [`Engine::dispatch_command`].
static COMMANDS: &[(&str, Handler)] = &[
    ("abort_dialog", Engine::cmd_abort_dialog),
    (
        "activechatphrase_prepare",
        Engine::cmd_activechatphrase_prepare,
    ),
    ("activechatphrase_send", Engine::cmd_activechatphrase_send),
    (
        "activechatphrase_sendprivate",
        Engine::cmd_activechatphrase_sendprivate,
    ),
    (
        "activechatphrase_setdynamicint",
        Engine::cmd_activechatphrase_setdynamic,
    ),
    (
        "activechatphrase_setdynamicobj",
        Engine::cmd_activechatphrase_setdynamic,
    ),
    ("applet_hasfocus", Engine::cmd_applet_hasfocus),
    ("automatedtestflags", Engine::cmd_automatedtestflags),
    ("chat_clear", Engine::cmd_chat_clear),
    ("chat_getfilter_private", Engine::cmd_chat_getfilter),
    ("chat_getfilter_public", Engine::cmd_chat_getfilter),
    ("chat_getfilter_trade", Engine::cmd_chat_getfilter),
    (
        "chat_gethistory_bytypeandline",
        Engine::cmd_chat_gethistory_by,
    ),
    ("chat_gethistory_byuid", Engine::cmd_chat_gethistory_by),
    ("chat_gethistorylength", Engine::cmd_chat_get),
    ("chat_getnextuid", Engine::cmd_chat_get),
    ("chat_getprevuid", Engine::cmd_chat_get),
    ("chat_lastuid", Engine::cmd_chat_lastuid),
    ("chat_playername", Engine::cmd_chat_playername),
    ("chat_playername_unfiltered", Engine::cmd_chat_playername),
    ("chat_sendabusereport", Engine::cmd_chat_sendabusereport),
    ("chat_sendprivate", Engine::cmd_chat_sendprivate),
    ("chat_sendpublic", Engine::cmd_chat_sendpublic),
    ("chat_setfilter", Engine::cmd_chat_setfilter),
    ("chat_setmode", Engine::cmd_chat_setmode),
    ("chatcat_findphrasebyshortcut", Engine::cmd_chatcat_find),
    ("chatcat_findsubcatbyshortcut", Engine::cmd_chatcat_find),
    ("chatcat_getdesc", Engine::cmd_chatcat_getdesc),
    ("chatcat_getphrase", Engine::cmd_chatcat_getsubcat),
    ("chatcat_getphrasecount", Engine::cmd_chatcat_get),
    (
        "chatcat_getphraseshortcut",
        Engine::cmd_chatcat_getsubcatshortcut,
    ),
    ("chatcat_getsubcat", Engine::cmd_chatcat_getsubcat),
    ("chatcat_getsubcatcount", Engine::cmd_chatcat_get),
    (
        "chatcat_getsubcatshortcut",
        Engine::cmd_chatcat_getsubcatshortcut,
    ),
    ("chatphrase_find", Engine::cmd_chatphrase_find),
    ("chatphrase_findnext", Engine::cmd_chatphrase_findnext),
    ("chatphrase_findrestart", Engine::cmd_chatphrase_findrestart),
    (
        "chatphrase_getautoresponse",
        Engine::cmd_chatphrase_getautoresponse,
    ),
    (
        "chatphrase_getautoresponsecount",
        Engine::cmd_chatphrase_getautoresponse,
    ),
    (
        "chatphrase_getdynamiccommand",
        Engine::cmd_chatphrase_getdynamiccommand,
    ),
    (
        "chatphrase_getdynamiccommandcount",
        Engine::cmd_chatphrase_getdynamiccommandcount,
    ),
    (
        "chatphrase_getdynamiccommandparam_enum",
        Engine::cmd_chatphrase_getdynamiccommandparam_enum,
    ),
    ("chatphrase_gettext", Engine::cmd_chatphrase_gettext),
    ("coord_fine", Engine::cmd_coord_fine),
    ("coord_finetogrid", Engine::cmd_coordx_fine),
    ("coord_gridtofine", Engine::cmd_coord_gridtofine),
    ("coordlevel_fine", Engine::cmd_coordx_fine),
    ("coordx_fine", Engine::cmd_coordx_fine),
    ("coordy_fine", Engine::cmd_coordx_fine),
    ("coordz_fine", Engine::cmd_coordx_fine),
    (
        "create_availablerequest",
        Engine::cmd_create_availablerequest,
    ),
    ("create_connect_reply", Engine::cmd_create_connect_reply),
    ("create_connectrequest", Engine::cmd_create_connectrequest),
    ("create_createrequest", Engine::cmd_create_createrequest),
    (
        "create_email_validate_reply",
        Engine::cmd_create_email_validate_reply,
    ),
    (
        "create_name_availablerequest",
        Engine::cmd_create_name_availablerequest,
    ),
    (
        "create_name_validate_reply",
        Engine::cmd_create_name_validate_reply,
    ),
    ("create_reply", Engine::cmd_create_reply),
    ("create_setunder13", Engine::cmd_create_setunder13),
    ("create_step_reached", Engine::cmd_create_step_reached),
    (
        "create_suggest_name_reply",
        Engine::cmd_create_suggest_name_reply,
    ),
    (
        "create_suggest_name_request",
        Engine::cmd_create_suggest_name_request,
    ),
    ("create_under13", Engine::cmd_create_under13),
    ("date_runeday", Engine::cmd_date_runeday),
    (
        "detailget_chosesafemode",
        Engine::cmd_detailget_chosesafemode,
    ),
    ("detailget_safemode", Engine::cmd_detailget_safemode),
    ("escape", Engine::cmd_escape),
    ("fullscreen_getmode", Engine::cmd_fullscreen_getmode),
    ("fullscreen_lastmode", Engine::cmd_fullscreen_lastmode),
    ("fullscreen_modecount", Engine::cmd_fullscreen_modecount),
    (
        "get_active_minimenu_entry",
        Engine::cmd_get_active_minimenu_entry,
    ),
    ("get_col_tag", Engine::cmd_get_col_tag),
    (
        "get_entity_bounding_box",
        Engine::cmd_get_entity_bounding_box,
    ),
    (
        "get_entity_overlay_height",
        Engine::cmd_get_entity_overlay_height,
    ),
    ("get_entity_say", Engine::cmd_get_entity_say),
    (
        "get_entity_screen_position",
        Engine::cmd_get_entity_screen_position,
    ),
    ("get_loc_bounding_box", Engine::cmd_get_entity_bounding_box),
    (
        "get_loc_overlay_height",
        Engine::cmd_get_entity_overlay_height,
    ),
    (
        "get_loc_screen_position",
        Engine::cmd_get_entity_screen_position,
    ),
    ("get_minimenu_length", Engine::cmd_get_minimenu_length),
    ("get_mousebuttons", Engine::cmd_get_mousebuttons),
    ("get_mousex", Engine::cmd_get_mousex),
    ("get_mousey", Engine::cmd_get_mousey),
    ("get_npc_name", Engine::cmd_get_npc_name),
    ("get_npc_stat", Engine::cmd_get_npc_stat),
    ("get_npc_vislevel", Engine::cmd_get_npc_vislevel),
    ("get_obj_bounding_box", Engine::cmd_get_entity_bounding_box),
    (
        "get_obj_overlay_height",
        Engine::cmd_get_entity_overlay_height,
    ),
    (
        "get_obj_screen_position",
        Engine::cmd_get_entity_screen_position,
    ),
    (
        "get_second_minimenu_entry",
        Engine::cmd_get_active_minimenu_entry,
    ),
    (
        "getgridcoordrelativetocamera",
        Engine::cmd_getgridcoordrelativetocamera,
    ),
    ("getwindowmode", Engine::cmd_getwindowmode),
    ("inv_freespace", Engine::cmd_inv_freespace),
    ("inv_getnum", Engine::cmd_inv_getobj),
    ("inv_getobj", Engine::cmd_inv_getobj),
    ("inv_getvar", Engine::cmd_inv_getvar),
    ("inv_size", Engine::cmd_inv_size),
    ("inv_total", Engine::cmd_inv_getobj),
    ("inv_totalparam", Engine::cmd_inv_totalparam),
    ("inv_totalparam_stack", Engine::cmd_inv_totalparam),
    ("invother_getnum", Engine::cmd_inv_getobj),
    ("invother_getobj", Engine::cmd_inv_getobj),
    ("invother_getvar", Engine::cmd_inv_getvar),
    ("invother_total", Engine::cmd_inv_getobj),
    ("is_npc_active", Engine::cmd_is_npc),
    ("is_npc_visible", Engine::cmd_is_npc),
    ("is_targeted_entity", Engine::cmd_is_targeted_entity),
    ("keyheld_alt", Engine::cmd_keyheld),
    ("keyheld_ctrl", Engine::cmd_keyheld),
    ("keyheld_shift", Engine::cmd_keyheld),
    ("lastlogin", Engine::cmd_lastlogin),
    ("lc_param", Engine::cmd_lc_param),
    ("lobby_entergame", Engine::cmd_lobby_entergame),
    ("lobby_entergamereply", Engine::cmd_login_reply),
    ("lobby_enterlobby", Engine::cmd_login_request),
    ("lobby_enterlobby_sso", Engine::cmd_lobby_enterlobby_sso),
    ("lobby_enterlobbyreply", Engine::cmd_lobby_enterlobbyreply),
    ("lobby_leavelobby", Engine::cmd_lobby_leavelobby),
    ("login_ban_duration", Engine::cmd_login_ban_duration),
    ("login_cancel", Engine::cmd_login_cancel),
    ("login_disallowresult", Engine::cmd_login_disallowresult),
    ("login_disallowtrigger", Engine::cmd_login_disallowtrigger),
    ("login_hoptime", Engine::cmd_login_hoptime),
    ("login_inprogress", Engine::cmd_login_inprogress),
    (
        "login_last_transfer_reply",
        Engine::cmd_login_last_transfer_reply,
    ),
    ("login_queue_position", Engine::cmd_login_queue_position),
    ("login_reply", Engine::cmd_login_reply),
    ("login_request", Engine::cmd_login_request),
    ("login_resetreply", Engine::cmd_login_resetreply),
    ("lowercase", Engine::cmd_lowercase),
    ("map_members", Engine::cmd_map_members),
    ("map_quickchat", Engine::cmd_map_quickchat),
    ("map_world", Engine::cmd_map_world),
    ("marketing_init", Engine::cmd_marketing_init),
    ("marketing_sendevent", Engine::cmd_marketing_sendevent),
    ("mes", Engine::cmd_mes),
    ("movecoord_fine", Engine::cmd_coordx_fine),
    ("nc_param", Engine::cmd_nc_param),
    ("noopWorldMapCommand", Engine::cmd_noop_world_map_command),
    ("notifications_init", Engine::cmd_marketing_init),
    ("notifications_opensettings", Engine::cmd_marketing_init),
    ("npc_type", Engine::cmd_npc_type),
    ("oc_category", Engine::cmd_oc_category),
    ("oc_members", Engine::cmd_oc_members),
    ("oc_minimenu_colour", Engine::cmd_oc_minimenu_colour),
    (
        "oc_minimenu_colour_overridden",
        Engine::cmd_oc_minimenu_colour_overridden,
    ),
    (
        "player_group_member_get_join_xp",
        Engine::cmd_player_group_member_get_join_xp,
    ),
    (
        "player_group_member_get_same_world_var",
        Engine::cmd_player_group_member_get_same_world_var,
    ),
    ("playermember", Engine::cmd_playermember),
    ("playermod", Engine::cmd_playermod),
    ("playermodlevel", Engine::cmd_playermod),
    ("random", Engine::cmd_random),
    ("random_sound_pitch", Engine::cmd_random_sound_pitch),
    ("randominc", Engine::cmd_random),
    ("reboottimer", Engine::cmd_reboottimer),
    ("removetags", Engine::cmd_removetags),
    ("resume_countdialog", Engine::cmd_resume_countdialog),
    ("resume_hsldialog", Engine::cmd_resume_hsldialog),
    ("resume_namedialog", Engine::cmd_resume_namedialog),
    ("resume_objdialog", Engine::cmd_resume_objdialog),
    ("resume_stringdialog", Engine::cmd_resume_stringdialog),
    ("seq_param", Engine::cmd_seq_param),
    ("setdefaultcursors", Engine::cmd_setdefaultcursors),
    ("sethardcodedopcursors", Engine::cmd_sethardcodedopcursors),
    ("sound_distancefocusfilter_setparams", Engine::cmd_sound),
    ("sound_group_start", Engine::cmd_sound),
    ("sound_group_stop", Engine::cmd_sound),
    ("sound_jingle", Engine::cmd_sound),
    ("sound_jingle_volume", Engine::cmd_sound),
    ("sound_mixbuss_add", Engine::cmd_sound),
    ("sound_mixbuss_setlevel", Engine::cmd_sound),
    ("sound_song", Engine::cmd_sound),
    ("sound_song_stop", Engine::cmd_sound),
    ("sound_song_volume", Engine::cmd_sound),
    ("sound_speech_volume", Engine::cmd_sound),
    ("sound_synth", Engine::cmd_sound),
    ("sound_synth_rate", Engine::cmd_sound),
    ("sound_synth_volume", Engine::cmd_sound),
    ("sound_vorbis_rate", Engine::cmd_sound),
    ("sound_vorbis_volume", Engine::cmd_sound),
    ("sound_vorbis_volume_rate_group", Engine::cmd_sound),
    ("sso_available", Engine::cmd_sso_available),
    ("sso_displayname", Engine::cmd_sso_displayname),
    ("staffmodlevel", Engine::cmd_staffmodlevel),
    (
        "stockmarket_getoffercompletedcount",
        Engine::cmd_stockmarket,
    ),
    ("stockmarket_getoffercompletedgold", Engine::cmd_stockmarket),
    ("stockmarket_getoffercount", Engine::cmd_stockmarket),
    ("stockmarket_getofferitem", Engine::cmd_stockmarket),
    ("stockmarket_getofferprice", Engine::cmd_stockmarket),
    ("stockmarket_getoffertype", Engine::cmd_stockmarket),
    ("stockmarket_isofferadding", Engine::cmd_stockmarket),
    ("stockmarket_isofferempty", Engine::cmd_stockmarket),
    ("stockmarket_isofferfinished", Engine::cmd_stockmarket),
    ("stockmarket_isofferstable", Engine::cmd_stockmarket),
    (
        "telemetry_get_group_count",
        Engine::cmd_telemetry_get_group_count,
    ),
    ("tostring_localised", Engine::cmd_tostring_localised),
    ("userdetail_dob", Engine::cmd_userdetail_dob),
    (
        "userdetail_lobby_ccexpiry",
        Engine::cmd_userdetail_lobby_ccexpiry,
    ),
    (
        "userdetail_lobby_dobrequested",
        Engine::cmd_userdetail_lobby_dobrequested,
    ),
    (
        "userdetail_lobby_emailstatus",
        Engine::cmd_userdetail_lobby_emailstatus,
    ),
    (
        "userdetail_lobby_graceexpiry",
        Engine::cmd_userdetail_lobby_graceexpiry,
    ),
    (
        "userdetail_lobby_jcoins_balance",
        Engine::cmd_userdetail_lobby_jcoins_balance,
    ),
    ("userdetail_lobby_lastloginaddress", Engine::cmd_lastlogin),
    (
        "userdetail_lobby_lastloginday",
        Engine::cmd_userdetail_lobby_lastloginday,
    ),
    (
        "userdetail_lobby_loyalty_balance",
        Engine::cmd_userdetail_lobby_loyalty_balance,
    ),
    (
        "userdetail_lobby_membership",
        Engine::cmd_userdetail_lobby_membership,
    ),
    (
        "userdetail_lobby_membersstats",
        Engine::cmd_userdetail_lobby_membersstats,
    ),
    (
        "userdetail_lobby_playage",
        Engine::cmd_userdetail_lobby_playage,
    ),
    (
        "userdetail_lobby_recoveryday",
        Engine::cmd_userdetail_lobby_recoveryday,
    ),
    (
        "userdetail_lobby_unreadmessages",
        Engine::cmd_userdetail_lobby_unreadmessages,
    ),
    ("userdetail_quickchat", Engine::cmd_userdetail_quickchat),
    ("userflowflags", Engine::cmd_userflowflags),
    ("worldlist_autoworld", Engine::cmd_worldlist_autoworld),
    ("worldlist_fetch", Engine::cmd_worldlist_fetch),
    ("worldlist_next", Engine::cmd_worldlist),
    ("worldlist_pingworlds", Engine::cmd_worldlist_pingworlds),
    ("worldlist_sort", Engine::cmd_worldlist_sort),
    ("worldlist_specific", Engine::cmd_worldlist),
    (
        "worldlist_specific_thisworld",
        Engine::cmd_worldlist_specific_thisworld,
    ),
    ("worldlist_start", Engine::cmd_worldlist),
    ("worldlist_switch", Engine::cmd_worldlist_switch),
    ("worldmap_3dview_active", Engine::cmd_worldmap_3dview_active),
];

/// The owner dispatchers in their `trap_context` order.
static OWNERS: &[(&str, Owner)] = &[
    ("social_mutation", Engine::owner_social_mutation),
    ("obj_queries", Engine::owner_obj_queries),
    ("bas", Engine::owner_bas),
    ("stockmarket", Engine::owner_stockmarket),
    ("db", Engine::owner_db),
    ("world_map", Engine::owner_world_map),
    ("cam2", Engine::owner_cam2),
    ("social", Engine::owner_social),
    ("builtins", Engine::owner_builtins),
    ("host_game", Engine::owner_host_game),
];

impl Engine {
    /// `trap_context`: the table handler for `c.command`, else the owners.
    pub(super) fn dispatch_command(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        if let Ok(i) =
            COMMANDS.binary_search_by(|(name, _)| name.as_bytes().cmp(c.command.as_bytes()))
        {
            return (COMMANDS[i].1)(self, c, ints, objs, longs);
        }
        self.dispatch_owners(c, ints, objs, longs)
    }

    /// The owners in order, then the unsupported-command count and error.
    fn dispatch_owners(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> VmResult<Option<Value>> {
        for (_, owner) in OWNERS {
            if let Some(result) = owner(self, c, ints, objs, longs) {
                return result;
            }
        }
        *self.unsupported.entry(c.command.into()).or_default() += 1;
        Err(absent(c.command))
    }
}

impl Engine {
    fn owner_social_mutation(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        crate::ui_social::State::handles_mutation(c.command)
            .then(|| self.cmd_social_mutation(c, ints, objs, longs))
    }

    fn owner_obj_queries(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        crate::ui_obj_queries::dispatch(self, c.command, ints)
    }

    fn owner_bas(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        _objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        crate::ui_bas::dispatch(self, c.command, ints)
    }

    fn owner_stockmarket(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        c.command
            .starts_with("stockmarket_")
            .then(|| self.cmd_stockmarket(c, ints, objs, longs))
    }

    fn owner_db(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        self.configs.db.dispatch(c.command, ints, objs)
    }

    fn owner_world_map(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        self.world_map_command(c.command, ints, objs).transpose()
    }

    fn owner_cam2(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        let active_npc = match self.scene.active_entity.as_ref() {
            Some(ActiveEntity::Npc { index, .. }) => Some(*index),
            _ => None,
        };
        self.camera
            .cam2
            .dispatch_active_with_objects(c.command, ints, objs, active_npc)
    }

    fn owner_social(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        crate::ui_social::State::handles(c.command)
            .then(|| self.social.dispatch(c.command, ints, objs))
    }

    fn owner_builtins(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        self.host_builtin(c, ints, objs, longs)
    }

    fn owner_host_game(
        &mut self,
        c: &InstructionContext<'_>,
        ints: &mut Vec<i32>,
        objs: &mut Vec<String>,
        _longs: &mut Vec<i64>,
    ) -> Option<VmResult<Option<Value>>> {
        host_game::dispatch(self, c, ints, objs)
    }
}
