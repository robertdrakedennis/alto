//! Config queries, installer opt-in and native platform cases.

use super::super::host_builtins::{SetupLauncher, SetupStatus};

use rs910_core::fault::Fault;

use std::{path::PathBuf, rc::Rc};

use super::{
    c, local_date, local_year, os_name, rec, soundflower_running, system_clipboard, wall_millis,
    Case, Obs, Set, World,
};

/// Config-list queries (`Client.*TypeList.list` decodes defaults for an
/// absent file).
pub(super) fn config_queries() -> Vec<Case> {
    fn elements(w: &mut World) {
        let mut store = crate::minimap::MapElementStore::default();
        // opcode 3 text "Bank", 6 textSize 2, 19 category 7, 249 int param 12 = 9.
        let element = crate::minimap::MapElement::decode(&[
            3, b'B', b'a', b'n', b'k', 0, 6, 2, 19, 0, 7, 249, 1, 0, 0, 0, 12, 0, 0, 0, 9, 0,
        ])
        .unwrap();
        store.types.insert(5, element);
        w.engine.configs.map_element_types = Some(store);
    }
    fn objs(w: &mut World) {
        let obj = |id: u32, name: &str, extra: &[u8]| {
            let mut bytes = vec![2];
            bytes.extend(name.bytes());
            bytes.push(0);
            bytes.extend_from_slice(extra);
            bytes.push(0);
            (id, crate::config::decode_obj(id, &bytes).unwrap())
        };
        // Opcode 65 = stockmarket; 94 category (u16).
        w.engine.configs.objs = Some(Rc::new(crate::config::ObjStore::from_map(
            [
                obj(0, "Rune sword", &[65, 94, 0, 3]),
                obj(1, "Bronze sword", &[94, 0, 3]),
                obj(2, "Iron sword", &[65]),
                obj(3, "Shield", &[65]),
            ]
            .into_iter()
            .collect(),
        )));
    }
    vec![
        // mec_text/sprite/textsize/category and mec_param
        //  (ParamConfig default 0 for an absent param 13).
        c("mec_text").with(elements).i(&[5]).ws(&["Bank"]),
        c("mec_textsize").with(elements).i(&[5]).wi(&[2]),
        c("mec_category").with(elements).i(&[5]).wi(&[7]),
        c("mec_sprite").with(elements).i(&[6]).wi(&[-1]),
        c("mec_param").with(elements).i(&[5, 12]).wi(&[9]),
        c("mec_param").with(elements).i(&[5, 13]).wi(&[0]),
        // Object search: "SWORD" matches 0..2 by name, sorted by name →
        // Bronze(1), Iron(2), Rune(0).
        c("oc_find")
            .with(objs)
            .i(&[0])
            .s(&["SWORD"])
            .wi(&[3])
            .check(|w| {
                let r = &w.engine.game_host.obj_find_results;
                (r.as_deref() == Some(&[1, 2, 0][..]) && w.engine.game_host.obj_find_index == 0)
                    .then_some(())
                    .ok_or(format!("{r:?}"))
            }),
        // Tradeable only (stockmarket): Rune and Iron.
        c("oc_find").with(objs).i(&[1]).s(&["sword"]).wi(&[2]),
        rec("oc_findnext")
            .set(Set::ObjFind(&[1, 2, 0], 1))
            .obs(Obs::ObjFindIndex),
        rec("oc_findnext")
            .set(Set::ObjFind(&[1, 2, 0], 3))
            .obs(Obs::ObjFindIndex),
        rec("oc_findrestart")
            .set(Set::ObjFind(&[1, 2, 0], 2))
            .obs(Obs::ObjFindIndex),
        // inv_stockbase: inventory type 7 stock {(995, 50), (1, 3)}.
        c("inv_stockbase")
            .with(|w| {
                let inv =
                    crate::config::decode_inv(7, &[4, 2, 3, 0xE3, 0, 50, 0, 1, 0, 3, 0]).unwrap();
                w.engine.game_host.inv_types = Some(crate::config::InvStore::from_map(
                    [(7, inv)].into_iter().collect(),
                ));
            })
            .i(&[7, 1])
            .wi(&[3]),
        c("inv_stockbase")
            .with(|w| {
                let inv = crate::config::decode_inv(7, &[4, 1, 3, 0xE3, 0, 50, 0]).unwrap();
                w.engine.game_host.inv_types = Some(crate::config::InvStore::from_map(
                    [(7, inv)].into_iter().collect(),
                ));
            })
            .i(&[7, rs910_symbols::obj::ABYSSAL_WHIP.id()])
            .wi(&[-1]),
        // getCategoryCount: inventory 93 holds
        // obj 0 (category 3) x5, obj 2 (category -1... default) x1, obj 1
        // (category 3) x2 → 7 for category 3.
        c("inv_totalcat")
            .with(|w| {
                objs(w);
                w.engine.inv_cache.update(93, 0, 0, 5, false);
                w.engine.inv_cache.update(93, 1, 2, 1, false);
                w.engine.inv_cache.update(93, 2, 1, 2, false);
            })
            .i(&[93, 3])
            .wi(&[7]),
        c("inv_totalcat")
            .i(&[94, 3])
            .wi(&[0])
            .note("no such inventory: 0"),
        // seqlength: postDecode sums frame lengths
        // (5 + 7 + 11); an absent id decodes the default (length 0).
        c("seqlength").with(seqs).i(&[808]).wi(&[23]),
        c("seqlength").with(seqs).i(&[809]).wi(&[0]),
    ]
}

pub(super) fn seqs(w: &mut World) {
    // Opcode 1: 3 frames, lengths 5/7/11, ids 1/2/3, high words 0.
    let seq = crate::config::decode_seq(
        808,
        &[
            1, 0, 3, 0, 5, 0, 7, 0, 11, 0, 1, 0, 2, 0, 3, 0, 0, 0, 0, 0, 0, 0,
        ],
    )
    .unwrap();
    w.engine.configs.seqs = Some(Rc::new(crate::config::SeqStore::from_map(
        [(808, seq)].into_iter().collect(),
    )));
}

/// The live-streaming platform probe and the installer launcher. The
/// installer rows were recorded with the original client's host OS set to the
/// named system (`os.name`); the installer is a shell script beside the cache
/// directory, so the rows that run it exist on Unix hosts only.
pub(super) fn setup_launcher() -> Vec<Case> {
    let mut v = vec![
        rec("ttv_library_request")
            .obs(Obs::Twitch)
            .note("no streaming SDK ships with this client: the platform probe reads unsupported"),
        rec("saveRuneScapeSetup")
            .with(|w| w.engine.builtins.setup = SetupLauncher::at("linux", None))
            .obs(Obs::Setup)
            .note("only Windows has an installer"),
        rec("saveRuneScapeSetup")
            .with(|w| w.engine.builtins.setup = SetupLauncher::at("windows", Some(scratch_dir())))
            .obs(Obs::Setup)
            .note("the installer is missing"),
    ];
    #[cfg(unix)]
    v.extend([
        rec("saveRuneScapeSetup")
            .with(|w| install_setup(w, "exit 0", false))
            .obs(Obs::Setup)
            .note("the installer starts"),
        rec("pushRuneScapeSetupValue")
            .with(|w| install_setup(w, "exit 0", true))
            .note("the installer finished"),
        rec("pushRuneScapeSetupValue")
            .with(|w| install_setup(w, "exit 3", true))
            .note("the installer failed"),
        rec("saveRuneScapeSetup")
            .with(|w| {
                install_setup(w, "sleep 5", false);
                w.engine.builtins.setup.launch().unwrap();
            })
            .obs(Obs::Setup)
            .note("a second launch while the installer runs fails"),
    ]);
    v
}

/// An installer beside the cache is never started unless the user opted in:
/// the launch fails as if the file were missing and nothing runs.
#[cfg(unix)]
#[test]
pub(super) fn installer_is_not_started_without_the_opt_in() {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir();
    let marker = dir.join("started");
    let exe = dir.join("RuneScape-Setup.exe");
    std::fs::write(&exe, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut w = World::new();
    w.engine.builtins.setup = SetupLauncher::at("windows", Some(dir));
    w.run(&c("saveRuneScapeSetup"));
    let outcome = w.outcome.as_ref().unwrap();
    assert!(
        outcome
            .error
            .as_deref()
            .is_some_and(|e| e.contains("missing")),
        "{outcome:?}"
    );
    assert_eq!(w.engine.builtins.setup.state(), SetupStatus::Idle);
    assert!(!marker.exists());
}

/// A fresh empty directory for one case.
pub(super) fn scratch_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cs2-setup-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A Windows host whose installer runs `body`; with `finish` the installer is
/// launched and has exited when the case starts.
#[cfg(unix)]
pub(super) fn install_setup(w: &mut World, body: &str, finish: bool) {
    use std::os::unix::fs::PermissionsExt;
    let dir = scratch_dir();
    let exe = dir.join("RuneScape-Setup.exe");
    std::fs::write(&exe, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    w.engine.builtins.setup = SetupLauncher::at("windows", Some(dir)).allowing_launch();
    if finish {
        w.engine.builtins.setup.launch().unwrap();
        let limit = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while w.engine.builtins.setup.status() == SetupStatus::Running {
            assert!(
                std::time::Instant::now() < limit,
                "the installer never exited"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

/// Platform/environment-dependent handlers: the expectation is derived from
/// the same platform facts the handlers read (OS name, the system clock, the
/// default time zone, the system clipboard).
pub(super) fn platform() -> Vec<Case> {
    let os = os_name();
    let mac = os.starts_with("mac");
    let win = os.starts_with("win");
    let flag = |b: bool| i32::from(b);
    vec![
        // osName (os.name lowercased).
        c("os_ismac").wi(&[flag(os.starts_with("mac"))]),
        c("os_iswindows").wi(&[flag(win)]),
        c("os_islinux").wi(&[flag(os.starts_with("linux"))]),
        // The machine probe's memory reaches the script queries: 16 GB
        // reads as installed and never offers safe mode; a machine under
        // 512 MB does, as does one already in safe mode.
        c("os_physicalmemorysize")
            .with(|w| w.engine.platform.physical_memory_mb = 16_384)
            .wi(&[16_384]),
        c("detailget_canchoosesafemode")
            .with(|w| w.engine.platform.physical_memory_mb = 16_384)
            .wi(&[0]),
        c("detailget_canchoosesafemode")
            .with(|w| w.engine.platform.physical_memory_mb = 256)
            .wi(&[1]),
        c("detailget_canchoosesafemode")
            .with(|w| {
                w.engine.platform.physical_memory_mb = 16_384;
                w.engine.platform.safe_mode = true;
            })
            .wi(&[1]),
        // isWebcamSupported: windows only (T106).
        c("ttv_webcam_supported").wi(&[flag(win)]),
        // hasPrerequisites (T267): mac → `ps -few` lists soundflowerbed.
        c("ttv_hasprerequisites").wi(&[flag(mac && soundflower_running())]),
        // date_minutes / date_year read
        // get() (wall-clock milliseconds).
        c("date_minutes").free().check(|w| {
            let got = w.outcome_int()?;
            let now = (wall_millis() / 60_000) as i32;
            ((now - 1..=now).contains(&got))
                .then_some(())
                .ok_or(format!("{got} vs {now}"))
        }),
        c("date_year").free().check(|w| {
            let got = w.outcome_int()?;
            let year = local_year();
            (got == year)
                .then_some(())
                .ok_or(format!("{got} vs {year}"))
        }),
        // fromdate formats in the default zone (dd-Mon-yyyy for English);
        // runeday 8000 at 00:00 UTC.
        c("fromdate")
            .free()
            .with(|w| w.vars.queries.language = Some(crate::ui_text_compare::Language::En))
            .i(&[8000])
            .check(|w| {
                let got = w.outcome_str()?;
                let want = local_date(i64::from(8000 + 11745) * 86_400);
                (got == want)
                    .then_some(())
                    .ok_or(format!("{got} vs {want}"))
            }),
        // getclipboard: the system clipboard's text flavour.
        c("getclipboard").free().check(|w| {
            let got = w.outcome_str()?;
            let want = system_clipboard();
            (got == want)
                .then_some(())
                .ok_or(format!("{got:?} vs {want:?}"))
        }),
        // profile times 10000 software triangles: a
        // non-negative elapsed millisecond count.
        c("profile_cpu").free().check(|w| {
            let got = w.outcome_int()?;
            (got >= 0).then_some(()).ok_or(format!("{got}"))
        }),
        // quit: the application environment exits.
        c("quit").check(|w| {
            (w.engine.builtins.requests == [super::super::host_builtins::Request::Quit])
                .then_some(())
                .ok_or(format!("{:?}", w.engine.builtins.requests))
        }),
        // docheat → doCheat(cheat, false, false).
        c("docheat").s(&["::tele 3200 3200"]).check(|w| {
            (w.engine.builtins.requests
                == [super::super::host_builtins::Request::Cheat(
                    "::tele 3200 3200".into(),
                )])
            .then_some(())
            .ok_or(format!("{:?}", w.engine.builtins.requests))
        }),
        // mes_typed: type 99 → addline, 98 →
        // the console entry line, else addMessage(type, ...).
        c("mes_typed").i(&[99, 0]).s(&["hello"]).check(|w| {
            (w.engine.effects.console_messages == ["hello"])
                .then_some(())
                .ok_or(format!("{:?}", w.engine.effects.console_messages))
        }),
        c("mes_typed").i(&[98, 0]).s(&["draft"]).check(|w| {
            (w.engine.builtins.requests
                == [super::super::host_builtins::Request::ConsoleEntry(
                    "draft".into(),
                )])
            .then_some(())
            .ok_or(format!("{:?}", w.engine.builtins.requests))
        }),
        c("mes_typed").i(&[2, 0]).s(&["hi there"]).check(|w| {
            let line = w.engine.messages.history.get_by_type_and_line(2, 0);
            (line.map(|l| (l.chat_type, l.message.as_str())) == Some((2, "hi there")))
                .then_some(())
                .ok_or(format!("{line:?}"))
        }),
        // openurlraw → Browser.openUrl(url, true, false):
        // Desktop.browse; flag 0 targets the applet frame (none).
        c("openurlraw").i(&[1]).s(&["http://a.ws"]).check(|w| {
            (w.engine.effects.browser_urls
                == [crate::server_prot::UiEvent::UrlOpen {
                    primary: "http://a.ws".into(),
                    fallback: None,
                    javascript: false,
                }])
            .then_some(())
            .ok_or(format!("{:?}", w.engine.effects.browser_urls))
        }),
        c("openurlraw").i(&[0]).s(&["http://a.ws"]).check(|w| {
            w.engine
                .effects
                .browser_urls
                .is_empty()
                .then_some(())
                .ok_or(format!("{:?}", w.engine.effects.browser_urls))
        }),
        // openurl_nologin: site URL + path, site settings "s1"; flag 0 pops both
        // arguments and opens nothing.
        c("openurl_nologin")
            .with(|w| w.engine.login.site_settings = "s1".into())
            .i(&[0])
            .s(&["a.ws"])
            .check(|w| {
                w.engine
                    .effects
                    .browser_urls
                    .is_empty()
                    .then_some(())
                    .ok_or(format!("{:?}", w.engine.effects.browser_urls))
            }),
        c("openurl_nologin")
            .with(|w| w.engine.login.site_settings = "s1".into())
            .i(&[1])
            .s(&["a.ws"])
            .check(|w| {
                (w.engine.effects.browser_urls
                    == [crate::server_prot::UiEvent::UrlOpen {
                        primary: "http://www.runescape.example.com/l=0/a=0/p=s1/a.ws".into(),
                        fallback: None,
                        javascript: false,
                    }])
                .then_some(())
                .ok_or(format!("{:?}", w.engine.effects.browser_urls))
            }),
        // setup_messagebox (the alignment values index the three loading
        // screen alignments).
        c("setup_messagebox")
            .i(&[1, 2, 10, 20, 300, 200, 1, 2, 3, 0xFFFFFF, 494])
            .check(|w| {
                let m = &w.engine.builtins.message_box;
                (m.setup
                    && (m.halign, m.valign) == (1, 2)
                    && m.box_xy == [10, 20]
                    && m.min_size == [300, 200]
                    && (m.border_corner, m.border_line, m.background) == (1, 2, 3)
                    && (m.colour, m.font) == (0xFFFFFF, 494))
                    .then_some(())
                    .ok_or(format!("{m:?}"))
            }),
        c("setup_messagebox")
            .i(&[3, 0, 10, 20, 300, 200, 1, 2, 3, 0xFFFFFF, 494])
            .throws(Fault::IndexOutOfRange),
    ]
}
