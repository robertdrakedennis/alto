//! Client fields, helper commands, platform probes and outgoing packet cases.

use super::{c, rec, test_fonts, Case, Obs, Set, C0, C1};

/// Disabled constant/discard handlers of both partitions: each pops exactly
/// its arguments and pushes its fixed values.
pub(super) fn disabled_handlers() -> Vec<Case> {
    vec![
        rec("applyDisplayPreference")
            .i(&[7, 1454, 1])
            .obs(Obs::Prefetch),
        rec("discardDetailSettingPair").i(&[4, 1]),
        rec("discardDisplayBounds").i(&[0, 0, 765, 503]),
        rec("discardDisplaySettingPair").i(&[2, 1]),
        rec("discardDragResizePair").i(&[10, 20]),
        rec("discardDragTarget").i(&[C1]),
        rec("discardDragTargetPair").i(&[C0, C1]),
        rec("discardFontArg").i(&[494]),
        rec("discardFourInterfaceArgs").i(&[C0, 1, 2, 3]),
        rec("discardNotificationGroup").s(&["friends"]),
        rec("discardWorldMapMenuAction").i(&[3, 1]).s(&["Travel"]),
        rec("disabled_command_1144").note("case 1144 returns without a call"),
        rec("noopAutosetupCommand"),
        rec("noopInterfaceCommand"),
        rec("noopLicenseCommand"),
        rec("noopTargetModeCommand"),
        rec("noopWalkMarkerCommand"),
        rec("pushFontAvailable"),
        rec("pushLicenseAvailable"),
        rec("pushNotificationPermissionGranted"),
        rec("pushNotificationUnavailable").i(&[3]),
        rec("pushShopUnavailable"),
        rec("pushTargetModeUnavailable"),
        rec("pushTargetModeZero"),
        rec("pushUnsupportedCommandDefaults").i(&[7]),
        rec("pushUnsupportedCommandTriple").i(&[1, 2]),
        rec("pushUnsupportedPairFalse").i(&[1, 2]),
        rec("pushUnsupportedPairFalseAlt").i(&[1, 2]),
        rec("pushUnsupportedPairFalseFifth").i(&[1, 2]),
        rec("pushUnsupportedPairFalseFourth").i(&[1, 2]),
        rec("pushUnsupportedPairFalseThird").i(&[1, 2]),
        rec("pushUnsupportedQuadFalse").i(&[1, 2, 3, 4]),
        rec("pushWorldMapUnavailable"),
        rec("pushZeroInsets"),
        rec("pushRuneScapeSetupValue").note("the setup status starts at its initial value"),
        rec("playerdemo"),
        rec("runjavascript"),
        rec("setwalkmarker").i(&[3222, 3218]),
        rec("show_software_license"),
        rec("notifications_cancellocal").i(&[12]),
        rec("notifications_sendgroupedlocal")
            .i(&[1, 60, 2])
            .s(&["title", "body", "group"]),
        rec("notifications_sendlocal")
            .i(&[1, 60])
            .s(&["title", "body"]),
        rec("shader_preload_allow").i(&[1]),
        rec("shader_preload_percent"),
        rec("shader_preload_throttle").i(&[50]),
        rec("battery_getlevelpercent"),
        rec("fps_stats").note(
            "the frame rate is 0 before the first redraw; the ring is a process global the \
             replay tests read, so no redraws are recorded here",
        ),
        rec("battery_ischarging"),
        rec("can_run_classic_client"),
        rec("has_html5"),
        rec("os_driver_outdated"),
        rec("os_driver_vendor"),
        rec("os_isandroid"),
        rec("os_isios"),
        rec("preload_download_complete"),
        rec("preload_download_downloadedsize"),
        rec("preload_download_rate"),
        rec("preload_download_remainingsize"),
        rec("preload_download_totalsize"),
        rec("video_advert_allow_skip").note("scripting is disabled"),
        rec("video_advert_force_remove"),
        rec("video_advert_has_finished"),
        rec("video_advert_play")
            .i(&[9])
            .note("pops only inside the javascript branch"),
        rec("notify_accountcreated"),
        rec("notify_accountcreatestarted"),
        rec("interface_getpickingradius"),
        rec("minimenu_close"),
        rec("shop_applypendingtransactions"),
        rec("shop_getcategorycount"),
        rec("shop_getcategorydescription").i(&[2]),
        rec("shop_getcategoryid").i(&[5]),
        rec("shop_getindexforcategoryid").i(&[5]),
        rec("shop_getindexforcategoryname").s(&["Weapons"]),
        rec("shop_getproductcount").i(&[0]),
        rec("shop_getproductdetails").i(&[0, 3]),
        rec("shop_isproductavailable").i(&[0, 3]),
        rec("shop_isproductrecommended").i(&[0, 3]),
        rec("shop_open").i(&[1]),
        rec("shop_opencategories").i(&[1, 2]),
        rec("shop_requestdata"),
        rec("shop_requestdatastatus"),
        rec("map_build_complete"),
        rec("map_loadedpercent"),
        rec("map_loadingscreen_isopen"),
        rec("map_loadingscreen_settriggerpercent").i(&[50, 1]),
        rec("map_preload").i(&[(3222 << 14) | 3218]),
        rec("worldmap_3dview_getloddistance"),
        rec("worldmap_3dview_getscreenposition").i(&[(3222 << 14) | 3218, 0]),
        rec("worldmap_3dview_gettextfont").i(&[1]),
        rec("worldmap_3dview_settextfont").i(&[1, 494]),
        rec("worldmap_getcategorypriority"),
        rec("worldmap_setcategorypriority"),
    ]
}

/// Pure helpers: coordinates, characters, trig, text, dates, colour.
pub(super) fn pure_helpers() -> Vec<Case> {
    let lumbridge = (3222 << 14) | 3218;
    vec![
        rec("coordx").i(&[(1 << 28) | (3200 << 14) | 3201]),
        rec("coordy").i(&[(3 << 28) | (3200 << 14) | 3201]),
        rec("coordz").i(&[(1 << 28) | (3200 << 14) | 3201]),
        rec("movecoord").i(&[lumbridge, 2, 1, -1]),
        rec("hsvtorgb").i(&[0x2E07]),
        rec("hsvtorgb")
            .i(&[-1])
            .note("only the low 16 bits index the table"),
        rec("sin_deg").i(&[2048]),
        rec("cos_deg").i(&[2048]),
        rec("atan2_deg").i(&[100, -50]),
        rec("char_isalpha").i(&['a' as i32]),
        rec("char_isalpha").i(&[0xE9]),
        rec("char_isalphanumeric").i(&['5' as i32]),
        rec("char_isalphanumeric").i(&['_' as i32]),
        rec("char_isnumeric").i(&['7' as i32]),
        rec("char_isnumeric").i(&['a' as i32]),
        rec("char_isprintable").i(&[0x20AC]),
        rec("char_isprintable").i(&[0x7F]),
        rec("char_isvalid").i(&[0x80]),
        rec("char_isvalid").i(&[' ' as i32]),
        rec("char_tolowercase").i(&[0xC9]),
        rec("char_touppercase").i(&[0xFF]),
        rec("string_distance").s(&["kitten", "sitting"]),
        rec("text_switch").i(&[1]).s(&["yes", "no"]),
        rec("text_switch").i(&[0]).s(&["yes", "no"]),
        rec("urlencode").s(&["a b*_.-Z9/?\u{20ac}\u{e9}"]),
        rec("clanforumqfc_tostring").l(&[1_234_567_890_123]),
        rec("clanforumqfc_tostring").l(&[-1]),
        rec("date_isleapyear").i(&[2000]),
        rec("date_isleapyear").i(&[1900]),
        rec("date_isleapyear").i(&[1500]),
        rec("date_minutes_fromruneday").i(&[8000]),
        rec("date_runeday_fromdate").i(&[27, 1, 2002]),
        rec("date_runeday_fromdate").i(&[31, 11, 1969]),
        rec("date_runeday_todate").i(&[8000]),
        rec("writeconsole").s(&["hello console"]),
    ]
}

/// Client fields set by the launcher/login owners.
pub(super) fn client_fields() -> Vec<Case> {
    vec![
        rec("affiliate").set(Set::Affiliate(1)),
        rec("frombilling").set(Set::Billing(true)),
        rec("playercountry").set(Set::Country(161)),
        rec("create_get_email").set(Set::Email("a@b.com")),
        rec("create_get_email"),
        rec("logout_getreason").note("the logout reason defaults to 0"),
        rec("is_gamescreen_state").set(Set::State(18)),
        rec("is_gamescreen_state"),
        rec("os_physicalmemorysize").note("physical memory reads 0 without the native probe"),
        rec("detailget_canchoosesafemode"),
        rec("get_currentcursor").set(Set::Cursor(5)),
        rec("map_isowner").set(Set::Owner("Zezima")).s(&["zEZIMA"]),
        rec("map_isowner").set(Set::Owner("Zezima")).s(&["Zezim"]),
        rec("login_accountappeal")
            .s(&["password"])
            .note("the appeal request returns 0 when the services host is unreachable"),
        rec("login_continue").note("login_continue only acts at login step 103"),
        rec("login_request_social_network")
            .i(&[1, 0])
            .s(&["auth"])
            .note("the login is not ready outside state 4"),
        rec("lobby_enterlobby_social_network")
            .i(&[1, 0])
            .s(&["auth"]),
        // A ready login takes the social network sign-on: no username or
        // password, the picked network, the login in progress with reply -3.
        c("login_request_social_network")
            .with(|w| w.engine.login.ready = true)
            .i(&[6, 1])
            .s(&["auth"])
            .check(|w| {
                let want = super::super::LoginRequest {
                    username: String::new(),
                    password: String::new(),
                    new_auth_preference: "auth".into(),
                    auth_dont_trust: true,
                    lobby: false,
                    sso: Some(6),
                };
                let login = &w.engine.login;
                (login.request.as_ref() == Some(&want)
                    && login.in_progress
                    && login.reply == -3
                    && !login.ready)
                    .then_some(())
                    .ok_or(format!("{:?}", login.request))
            }),
        c("lobby_enterlobby_social_network")
            .with(|w| w.engine.login.ready = true)
            .i(&[0, 0])
            .s(&[""])
            .check(|w| {
                let login = &w.engine.login;
                (login
                    .request
                    .as_ref()
                    .is_some_and(|r| r.lobby && r.sso == Some(0))
                    && login.in_progress
                    && login.lobby_reply == -3)
                    .then_some(())
                    .ok_or(format!("{:?}", login.request))
            }),
        c("login_continue").check(|w| {
            w.engine
                .login
                .continue_requested
                .then_some(())
                .ok_or("no continue request".to_owned())
        }),
        rec("resend_uid_passport_request")
            .obs(Obs::Out)
            .note("sends only in state 17"),
        // The device check's resend goes out only while a lobby login runs.
        c("resend_uid_passport_request")
            .with(|w| w.engine.login.lobby_logging_in = true)
            .check(|w| {
                w.engine
                    .login
                    .resend_uid_passport_requested
                    .then_some(())
                    .ok_or("no resend request".to_owned())
            }),
        rec("preload_percent").note("the resource providers do not exist yet"),
        rec("preload_progress"),
        c("preload_percent")
            .with(|w| w.engine.builtins.preload_progress = Some(100))
            .wi(&[100])
            .note("every provider prefetched: done * 100 / total = 100"),
        c("preload_progress")
            .with(|w| w.engine.builtins.preload_progress = Some(100))
            .wi(&[100]),
    ]
}

/// `TELEMETRY_GRID_FULL`: group 5, rows 11 (pinned) and 12, column 22;
/// row 11 holds 33, row 12 is null.
pub(super) const TELEMETRY: &[u8] = &[
    1, 0, 0, 0, 5, 2, 0, 0, 0, 11, 0, 0, 0, 12, 1, 0, 0, 0, 22, 1, 1, 0, 0, 0, 33, 0, 0,
];

pub(super) fn telemetry() -> Vec<Case> {
    let t = Set::Telemetry(TELEMETRY);
    vec![
        rec("telemetry_get_column_count").set(t).i(&[0]),
        rec("telemetry_get_column_id").set(t).i(&[0, 0]),
        rec("telemetry_get_column_index").set(t).i(&[0, 22]),
        rec("telemetry_get_grid_value").set(t).i(&[0, 0, 0]),
        rec("telemetry_get_grid_value").set(t).i(&[0, 1, 0]),
        rec("telemetry_get_group_id").set(t).i(&[0]),
        rec("telemetry_get_group_id").set(t).i(&[3]),
        rec("telemetry_get_group_index").set(t).i(&[5]),
        rec("telemetry_get_row_count").set(t).i(&[0]),
        rec("telemetry_get_row_id").set(t).i(&[0, 1]),
        rec("telemetry_get_row_index").set(t).i(&[0, 12]),
        rec("telemetry_is_grid_processor_set").set(t).i(&[0, 0, 0]),
        rec("telemetry_is_grid_processor_set").set(t).i(&[0, 1, 0]),
        rec("telemetry_is_row_pinned").set(t).i(&[0, 0]),
        rec("telemetry_is_row_pinned").set(t).i(&[0, 1]),
    ]
}

pub(super) fn emoji() -> Vec<Case> {
    const LONE_HIGH_SURROGATE: u16 = 0xd800;
    let lone_surrogate = native910::jstr::from_unit(LONE_HIGH_SURROGATE);
    let escaped_scalar = native910::jstr::from_text("\u{10f800}");
    let input = format!("{lone_surrogate} :) {escaped_scalar}");
    let substituted = format!("{lone_surrogate} <sprite=5> {escaped_scalar}");
    let smile = Set::Emoji(":)", 5, 0);
    vec![
        rec("emoji_add")
            .i(&[6, 2])
            .s(&[";p"])
            .obs(Obs::EmojiSub("hi ;p")),
        rec("emoji_add").i(&[6, 2]).s(&["a"]),
        rec("emoji_add").i(&[6, 2]).s(&[""]),
        rec("emoji_enable_auto_chatline")
            .i(&[1])
            .obs(Obs::EmojiAuto),
        rec("emoji_remove")
            .set(smile)
            .s(&[":)"])
            .obs(Obs::EmojiSub("a :)")),
        rec("emoji_removeall").set(smile).obs(Obs::EmojiSub("a :)")),
        rec("emoji_substitute").set(smile).s(&["hi :) <b:)> x"]),
        rec("emoji_substitute")
            .s(&["hi :)"])
            .note("an empty list leaves the argument"),
        c("emoji_substitute")
            .set(smile)
            .s(&[&input])
            .ws(&[&substituted])
            .note("substitution preserves all untouched UTF-16 units"),
    ]
}

/// Twitch commands: the native SDK never loads, so the SDK, the webcam
/// device list and the livestream list stay absent.
pub(super) fn twitch() -> Vec<Case> {
    vec![
        rec("ttv_chat_getstate"),
        rec("ttv_chat_sendmessage").s(&["hi"]),
        rec("ttv_library_getstate"),
        rec("ttv_livestreams_getstream_next"),
        rec("ttv_livestreams_getstream_next")
            .obs(Obs::TwitchCursor)
            .note("the live-stream cursor starts at -1"),
        rec("ttv_livestreams_getstream_start").obs(Obs::TwitchCursor),
        rec("ttv_livestreams_update"),
        rec("ttv_login").s(&["user", "pass"]),
        rec("ttv_login_getstate"),
        rec("ttv_logout"),
        rec("ttv_setdebugoutput").i(&[1, 0, 1]),
        rec("ttv_stream_getquality"),
        rec("ttv_stream_getstate"),
        rec("ttv_stream_getviewers"),
        rec("ttv_stream_setsmoothresize").i(&[0]).obs(Obs::Twitch),
        rec("ttv_stream_settitle").s(&["title"]),
        rec("ttv_stream_start").i(&[50, 50, 30, 1]),
        rec("ttv_stream_stop"),
        rec("ttv_webcam_flip").i(&[1, 0]).obs(Obs::Twitch),
        rec("ttv_webcam_getcap_byindex").i(&[0, 0]),
        rec("ttv_webcam_getcap_byuniqueid").i(&[0, 0]),
        rec("ttv_webcam_getcap_count").i(&[0]),
        rec("ttv_webcam_getdevice_byindex").i(&[0]),
        rec("ttv_webcam_getdevice_byuniquename").s(&["cam"]),
        rec("ttv_webcam_getdevice_count"),
        rec("ttv_webcam_getstate"),
        rec("ttv_webcam_start").i(&[-1, 0]).obs(Obs::Twitch),
        rec("ttv_webcam_start").i(&[0, 0]),
        rec("ttv_webcam_stop").i(&[-1]).obs(Obs::Twitch),
        rec("ttv_webcam_stop").i(&[0]),
        // Smooth resize: a change resets the capture (a toolkit call the
        // recording harness lacks) and stores the flag.
        c("ttv_stream_setsmoothresize")
            .i(&[1])
            .wo(Obs::Twitch, "0,false,false,true,false"),
    ]
}

/// Commands whose effect is an outgoing client packet.
pub(super) fn packets() -> Vec<Case> {
    let lumbridge = (3222 << 14) | 3218;
    vec![
        rec("email_validation_add_new_address")
            .i(&[1, 0, 1])
            .s(&["a@b.com"])
            .obs(Obs::Out),
        rec("email_validation_change_address")
            .s(&["old@b.com", "new@b.com"])
            .obs(Obs::Out),
        rec("email_validation_submit_code")
            .s(&["123456"])
            .obs(Obs::Out),
        rec("resume_clanforumqfcdialog").s(&["abc"]).obs(Obs::Out),
        rec("openurl")
            .i(&[1])
            .s(&["http://a.ws", "b"])
            .obs(Obs::Out),
        rec("openurl_shim")
            .i(&[0])
            .s(&["http://a.ws", "b", "c"])
            .obs(Obs::Out),
        rec("bug_report")
            .i(&[3])
            .s(&["title", "desc"])
            .obs(Obs::Out),
        rec("movescripted").i(&[1, lumbridge]).obs(Obs::Out),
        rec("movescripted").i(&[3, lumbridge]).obs(Obs::Out),
        rec("movescripted").i(&[1, -1]).obs(Obs::Out),
        // Nothing is queued while gameConnection is null.
        c("movescripted")
            .with(|w| w.engine.game_host.game_connection = false)
            .i(&[1, lumbridge])
            .wo(Obs::Out, ""),
        rec("affinedclansettings_addbanned_fromchannel")
            .set(Set::ClanUser("Bob", -1))
            .i(&[0])
            .obs(Obs::Out),
        rec("affinedclansettings_setmuted_fromchannel")
            .set(Set::ClanUser("Bob", 2))
            .i(&[0, 1])
            .obs(Obs::Out),
    ]
}

/// Commands whose owners the component hook host lends (canvas, minimenu,
/// preferences) plus the preference-domain toolkit commands.
pub(super) fn hook_owners() -> Vec<Case> {
    vec![
        rec("pushCanvasSize").set(Set::Canvas(765, 503)),
        rec("window_getinsets").set(Set::Canvas(765, 503)),
        rec("setsubmenuminlength").i(&[5]).obs(Obs::MinLength),
        rec("autosetup_blackflaglast")
            .set(Set::Autosetup(1))
            .obs(Obs::Blackflag),
        rec("autosetup_blackflaglast")
            .set(Set::Autosetup(3))
            .obs(Obs::Blackflag),
        // Metrics of the synthetic font 494 (see `test_fonts`): pushes the
        // four font metrics and the ascent and descent.
        c("pushFontMetrics")
            .with(test_fonts)
            .i(&[494])
            .wi(&[12, 9, 2, 3, 4]),
        // getPerformanceMetric: no benchmark model → 1.
        c("detailget_performance_metric")
            .with(|w| w.vars.queries.preferences.performance_metrics_model = Some(-1))
            .wi(&[1]),
        // detail_toolkit → setToolkit(0, false) →
        // changeToolkit/createToolkit: the software toolkit is
        // always available, so displayMode becomes 0 and the device change
        // is queued for the app's toolkit lifecycle (RecreateToolkit).
        c("detail_toolkit").i(&[0]).check(|w| {
            let p = &w.vars.queries.preferences;
            let mode = p.options.get("displayMode");
            let recreate = p
                .pending_effects
                .contains(&crate::ui_preferences::PreferenceEffect::RecreateToolkit);
            (mode == Some(0) && recreate).then_some(()).ok_or(format!(
                "displayMode {mode:?}, effects {:?}",
                p.pending_effects
            ))
        }),
        // text_gender: localPlayerEntity.model.isFemale picks
        // the second string.
        c("text_gender")
            .with(|w| w.local_player(true))
            .s(&["his", "her"])
            .ws(&["her"]),
        c("text_gender")
            .with(|w| w.local_player(false))
            .s(&["his", "her"])
            .ws(&["his"]),
    ]
}

/// The toolkit availability queries and the saved-toolkit setter on a machine
/// without DirectX (recorded from the original client on a host that never
/// fetched its DirectX library; this client is that machine on every
/// operating system). Toolkit 3 is refused, the default toolkit is GL (the
/// rows that read it start from a saved file that holds toolkit 1, which is
/// what a new file holds), and a saved file that holds toolkit 3 keeps it.
pub(super) fn toolkit_availability() -> Vec<Case> {
    vec![
        rec("detailcanset_toolkit_default").i(&[0]),
        rec("detailcanset_toolkit_default").i(&[1]),
        rec("detailcanset_toolkit_default").i(&[2]),
        rec("detailcanset_toolkit_default").i(&[3]),
        rec("detailcanset_toolkit_default").i(&[4]),
        rec("detailcanset_toolkit_default").i(&[5]),
        rec("detailcanmod_toolkit_default"),
        rec("detailget_toolkit").set(Set::LoadedToolkit(1)),
        rec("detailget_toolkit_default").set(Set::LoadedToolkit(1)),
        rec("detailget_toolkit").set(Set::LoadedToolkit(3)),
        rec("detailget_toolkit_default").set(Set::LoadedToolkit(3)),
        rec("detail_toolkit_default")
            .i(&[3, 1])
            .set(Set::LoadedToolkit(1))
            .obs(Obs::ToolkitPref),
        rec("detail_toolkit_default")
            .i(&[1, 1])
            .set(Set::LoadedToolkit(1))
            .obs(Obs::ToolkitPref),
        rec("detail_toolkit_default")
            .i(&[1, 1])
            .set(Set::LoadedToolkit(3))
            .obs(Obs::ToolkitPref),
        rec("detail_toolkit_default")
            .i(&[3, 1])
            .set(Set::LoadedToolkit(3))
            .obs(Obs::ToolkitPref),
    ]
}
