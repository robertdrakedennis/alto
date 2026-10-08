//! One test per settings surface, each run through the real cache interface:
//! the control's own hook fires, the saved option changes, the value the
//! client's consumers read changes with it, and the saved file reloads to the
//! same values through the options codec. The graphics panel (dropdowns,
//! checkboxes and the presets), the fullscreen and windowed buttons over a
//! scripted window, every family of the Controls tab and the Audio tab.
use super::settings_world::*;
use crate::client_options::{ClientOptions, LiveSettings};
use crate::rebuild::BuildPrefs;
use crate::ui_preferences::PreferenceEffect;
use rs910_symbols::component::{
    audio_settings, confirm_popup, controls_settings, game_window, graphics_change_confirm,
    graphics_settings_panel as panel,
};
use rs910_symbols::{interface, varp, ComponentId};

/// What the consumers read after a step.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    live: LiveSettings,
    build: BuildPrefs,
}

fn seen(world: &SettingsWorld) -> Seen {
    let options = world.options();
    Seen {
        live: options.live(),
        build: BuildPrefs::from_options(options),
    }
}

fn field(name: &str, world: &SettingsWorld) -> i32 {
    world.options().get(name).unwrap()
}

/// The saved file loads back to the values in memory.
fn assert_persisted(world: &mut SettingsWorld, what: &str) -> anyhow::Result<()> {
    let saved: ClientOptions = world.reload_options()?;
    assert_eq!(
        saved.encode(),
        world.options().encode(),
        "{what}: the saved options differ from the live ones"
    );
    Ok(())
}

fn has_effect(world: &SettingsWorld, effect: PreferenceEffect) -> bool {
    world.preferences().pending_effects.contains(&effect)
}

/// Every dropdown of the graphics panel that has an effect of its own, by
/// its position in the panel: display mode, CPU usage, roof removal, idle
/// animations, scenery shadows, lighting, water and particles.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_dropdowns_change_their_consumers() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("dropdowns", &[[1920, 1200]])?;
    world.open_tab(2)?;
    // Display mode: the first entry is the software toolkit; leaving the
    // current one queues a device recreation, and OpenGL brings it back. The
    // DirectX entry has its own test (it is unavailable by design).
    assert_eq!(field("displayMode", &world), 1);
    world.dropdown(0, 0)?;
    assert_eq!(field("displayMode", &world), 0);
    assert_eq!(field("toolkit", &world), 0);
    assert!(has_effect(&world, PreferenceEffect::RecreateToolkit));
    assert_persisted(&mut world, "display mode")?;
    world.preferences_mut().pending_effects.clear();
    world.dropdown(0, 1)?;
    assert_eq!(field("displayMode", &world), 1);
    assert!(has_effect(&world, PreferenceEffect::RecreateToolkit));

    let before = seen(&world);
    world.dropdown(1, 2)?;
    let after = seen(&world);
    assert_eq!((field("cpuUsage", &world), after.live.cpu_usage), (2, 2));
    assert_ne!(before.live.cpu_usage, after.live.cpu_usage);
    assert_persisted(&mut world, "cpu usage")?;

    world.dropdown(2, 1)?;
    assert_eq!(
        (field("removeRoofs", &world), field("removeRoofs2", &world)),
        (1, 1)
    );
    assert_eq!(seen(&world).live.roof_mode, 1);
    assert!(has_effect(&world, PreferenceEffect::SetupRoofs));
    assert_persisted(&mut world, "remove roofs")?;

    world.dropdown(3, 0)?;
    assert_eq!(seen(&world).live.idle_animation_detail, 0);
    assert_persisted(&mut world, "idle animations")?;

    world.dropdown(4, 1)?;
    let shadows = seen(&world);
    assert_eq!(
        (shadows.live.scenery_shadows, shadows.build.scenery_shadows),
        (1, 1)
    );
    assert!(has_effect(&world, PreferenceEffect::SceneRebuild));
    assert_persisted(&mut world, "scenery shadows")?;

    world.dropdown(5, 0)?;
    assert_eq!(seen(&world).build.lighting_detail, 0);
    assert!(has_effect(&world, PreferenceEffect::ResetModelCaches));
    assert_persisted(&mut world, "lighting detail")?;

    world.dropdown(6, 1)?;
    assert_eq!(seen(&world).build.water_detail, 2);
    assert_persisted(&mut world, "water detail")?;

    world.dropdown(8, 1)?;
    assert_eq!(seen(&world).live.particle_level, 1);
    assert_persisted(&mut world, "particles")?;
    Ok(())
}

/// DirectX is unavailable by design (the GL toolkit is the hardware toolkit).
/// The panel lists its entry as the original's does, since the list does not
/// ask the device; choosing it fails as on a machine without DirectX: the
/// failure is reported to the server, the toolkit that was active is created
/// again, the setting is written back to it and the player is told in the
/// chat box.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn directx_entry_fails_back_to_the_active_toolkit() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("directx", &[[1920, 1200]])?;
    world.open_tab(2)?;
    let failed = vec![crate::proto::client::SIMPLE_TOOLKIT_CHANGE, 128 + 4];
    let told = |world: &SettingsWorld| {
        let history = &world.ui.engine.messages.history;
        history
            .get_by_uid(history.last_uid())
            .map(|line| line.message.clone())
    };
    for (start, entry) in [(1, 1), (0, 0)] {
        world.dropdown(0, entry)?;
        assert_eq!(field("displayMode", &world), start);
        world.preferences_mut().pending_effects.clear();
        world.preferences_mut().graphics_packets.clear();
        // The list of display modes, opened again.
        world.op(panel::CONTROL_LIST, 0)?;
        let listed: Vec<String> = (0..8)
            .filter_map(|child| {
                let node = world
                    .ui
                    .store
                    .get(game_window::DROPDOWN_LIST.packed(), child)
                    .ok()??;
                let text = node.borrow().f.text.clone()?;
                Some(String::from_utf16_lossy(&text)).filter(|t| !t.is_empty())
            })
            .collect();
        assert!(listed.iter().any(|t| t == "DirectX\u{ae}"), "{listed:?}");
        world.pick(2)?;
        assert_eq!(
            (field("displayMode", &world), field("toolkit", &world)),
            (start, start),
            "the toolkit that was active is back"
        );
        assert_eq!(
            world.preferences().graphics_packets,
            std::slice::from_ref(&failed)
        );
        assert!(has_effect(&world, PreferenceEffect::RecreateToolkit));
        assert_eq!(
            told(&world).as_deref(),
            Some("RuneScape was unable to enter that display mode.")
        );
        assert_persisted(&mut world, "display mode")?;
    }
    Ok(())
}

/// The checkboxes: each flips its own option and the value its consumer
/// reads, and flips back.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_checkboxes_change_their_consumers() -> anyhow::Result<()> {
    type Read = fn(&Seen) -> i32;
    // (child of the panel, option, what the consumer reads). Textures come
    // before ground blending: switching blending off also switches them and
    // the scenery shadows off.
    let rows: &[(i32, &str, Read)] = &[
        (10, "groundDecoration", |s| s.build.ground_decoration),
        (11, "flickeringEffects", |s| i32::from(s.live.flickering)),
        (12, "characterShadows", |s| {
            i32::from(s.live.character_shadows)
        }),
        (13, "fog", |s| i32::from(s.live.fog)),
        (17, "skyboxes", |s| s.live.sky_detail),
        (16, "textures", |s| s.build.textures),
        (15, "groundBlending", |s| s.build.ground_blending),
    ];
    let mut world = SettingsWorld::login("checkboxes", &[[1920, 1200]])?;
    world.open_tab(2)?;
    for &(child, name, read) in rows {
        let start = seen(&world);
        world.op(panel::CONTROL_LIST, child)?;
        assert_eq!(field(name, &world), 0, "{name} did not switch off");
        assert_ne!(
            read(&start),
            read(&seen(&world)),
            "{name} did not reach its consumer"
        );
        assert_persisted(&mut world, name)?;
        if name != "groundBlending" {
            world.op(panel::CONTROL_LIST, child)?;
            assert_eq!(
                read(&seen(&world)),
                read(&start),
                "{name} did not switch back"
            );
            assert_eq!(field(name, &world), 1);
        }
    }
    Ok(())
}

/// The four presets and Custom.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_presets_reset_the_panel() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("presets", &[[1920, 1200]])?;
    world.open_tab(2)?;
    // (button, preset id, textures, water detail, scenery shadows, particles)
    for (button, preset, textures, water, shadows, particles) in [
        (panel::MIN_PRESET_BUTTON, 1, 0, 0, 0, 0),
        (panel::LOW_PRESET_BUTTON, 2, 0, 0, 0, 0),
        (panel::MEDIUM_PRESET_BUTTON, 3, 1, 0, 1, 1),
        (panel::HIGH_PRESET_BUTTON, 4, 1, 2, 2, 2),
    ] {
        world.op(button, -1)?;
        let s = seen(&world);
        assert_eq!(field("preset", &world), preset, "button {button:?}");
        assert_eq!(
            (
                s.build.textures,
                s.build.water_detail,
                s.build.scenery_shadows,
                s.live.particle_level
            ),
            (textures, water, shadows, particles),
            "button {button:?}"
        );
        assert!(has_effect(&world, PreferenceEffect::SceneRebuild));
        assert_persisted(&mut world, "preset")?;
    }
    // Custom keeps every graphics value and only clears the preset.
    let high = seen(&world);
    world.op(panel::CUSTOM_PRESET_BUTTON, -1)?;
    assert_eq!(field("preset", &world), 0);
    assert_eq!(seen(&world).build, high.build);
    assert_persisted(&mut world, "custom")?;
    // A changed control makes the setup Custom on its own.
    world.op(panel::HIGH_PRESET_BUTTON, -1)?;
    assert_eq!(field("preset", &world), 4);
    world.op(panel::CONTROL_LIST, 10)?;
    assert_eq!(field("preset", &world), 0);
    Ok(())
}

/// A monitor whose modes all lie above the Max Screen Size limit (macOS lists
/// only native-size modes) leaves the resolution list empty. The panel still
/// builds in full, with the fullscreen controls hidden, as it does on a
/// platform without fullscreen; the list query used to stop the whole build
/// at its first row.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_panel_builds_without_a_mode_inside_the_size_limit() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("no-mode", &[[3456, 2234], [2560, 1600], [1920, 1200]])?;
    world.preferences_mut().options.set_field("screenSize", 2);
    world.preferences_mut().window.forget_modes();
    world.open_tab(2)?;
    world.ticks(40)?;
    assert_eq!(
        world.ui.diagnostics.errors,
        Default::default(),
        "the panel's scripts ran clean"
    );
    // The preset buttons and the advanced rows come last in the build.
    assert_eq!(world.text(panel::MIN_PRESET_LABEL)?, "MIN");
    assert_eq!(world.text(panel::CUSTOM_PRESET_LABEL)?, "CUSTOM");
    Ok(())
}

/// The Fullscreen and Windowed buttons drive the native window through the
/// window owner: exclusive fullscreen at the chosen resolution, the
/// keep-or-revert confirmation, and the way back to the saved window mode.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn fullscreen_and_windowed_buttons_drive_the_window() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("window-keep", &[[1920, 1200], [1280, 720]])?;
    world.open_tab(2)?;
    assert_eq!(world.preferences().window.mode, 2);
    // The resolution list runs from the largest mode down: pick the second.
    world.op(panel::RESOLUTION_DROPDOWN, -1)?;
    world.pick(1)?;
    world.click(panel::FULLSCREEN_BUTTON)?;
    assert_eq!(world.window.calls(), [WindowCall::Fullscreen([1280, 720])]);
    assert_eq!(world.preferences().window.mode, 3);
    assert!(
        world.is_open(interface::GRAPHICS_CHANGE_CONFIRM),
        "the keep-this-setting dialog is up"
    );
    // Keeping it leaves the frame fullscreen and closes the dialog.
    world.op(graphics_change_confirm::YES_BUTTON, -1)?;
    assert!(!world.is_open(interface::GRAPHICS_CHANGE_CONFIRM));
    assert_eq!(world.preferences().window.mode, 3);
    assert_eq!(world.window.calls().len(), 1);
    // The Windowed button returns to the saved window mode and saves it.
    world.click(panel::WINDOWED_BUTTON)?;
    let calls = world.window.calls();
    assert_eq!(
        calls[1..],
        [WindowCall::Windowed, WindowCall::Resizable(true)]
    );
    assert_eq!(world.preferences().window.mode, 2);
    assert_eq!(field("windowMode", &world), 2);
    assert_persisted(&mut world, "window mode")?;
    Ok(())
}

/// Declining the confirmation, or letting it time out, puts the window back.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn fullscreen_reverts_when_declined_or_unanswered() -> anyhow::Result<()> {
    for answer in [Some(graphics_change_confirm::NO_BUTTON), None] {
        let mut world = SettingsWorld::login("window-revert", &[[1280, 720]])?;
        world.open_tab(2)?;
        world.click(panel::FULLSCREEN_BUTTON)?;
        assert_eq!(world.preferences().window.mode, 3);
        match answer {
            Some(no) => world.op(no, -1)?,
            // The dialog counts down 750 client cycles.
            None => world.ticks(800)?,
        }
        assert_eq!(world.preferences().window.mode, 2, "answer {answer:?}");
        assert_eq!(
            world.window.calls()[1..],
            [WindowCall::Windowed, WindowCall::Resizable(true)],
            "answer {answer:?}"
        );
        assert!(
            !world.is_open(interface::GRAPHICS_CHANGE_CONFIRM),
            "answer {answer:?}"
        );
    }
    Ok(())
}

/// A monitor that will not enter the mode leaves the window windowed and
/// reports the failure through the script.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn refused_fullscreen_keeps_the_window() -> anyhow::Result<()> {
    let mut world =
        SettingsWorld::login_on("window-refused", ScriptedWindow::refusing(&[[1280, 720]]))?;
    world.open_tab(2)?;
    world.click(panel::FULLSCREEN_BUTTON)?;
    assert_eq!(world.preferences().window.mode, 2);
    assert_eq!(
        world.window.calls(),
        [
            WindowCall::Fullscreen([1280, 720]),
            WindowCall::Windowed,
            WindowCall::Resizable(true)
        ]
    );
    assert!(!world.is_open(interface::GRAPHICS_CHANGE_CONFIRM));
    Ok(())
}

/// Losing focus in fullscreen, and the browser opening, restore the saved
/// window mode; a window that is not fullscreen is left alone.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn fullscreen_leaves_on_focus_loss_and_browser() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("window-focus", &[[1280, 720]])?;
    world.open_tab(2)?;
    let options = world.options().clone();
    assert!(!world.preferences_mut().window.focus_lost(&options, 18)?);
    world.click(panel::FULLSCREEN_BUTTON)?;
    world.op(graphics_change_confirm::YES_BUTTON, -1)?;
    assert_eq!(world.preferences().window.mode, 3);
    // Not in a game state: nothing happens.
    assert!(!world.preferences_mut().window.focus_lost(&options, 10)?);
    assert_eq!(world.preferences().window.mode, 3);
    assert!(world.preferences_mut().window.focus_lost(&options, 18)?);
    assert_eq!(world.preferences().window.mode, 2);
    assert_eq!(
        world.window.calls()[1..],
        [WindowCall::Windowed, WindowCall::Resizable(true)]
    );

    let mut world = SettingsWorld::login("window-browser", &[[1280, 720]])?;
    world.open_tab(2)?;
    let options = world.options().clone();
    assert!(!world.preferences_mut().window.leave_fullscreen(&options)?);
    world.click(panel::FULLSCREEN_BUTTON)?;
    world.op(graphics_change_confirm::YES_BUTTON, -1)?;
    assert!(world.preferences_mut().window.leave_fullscreen(&options)?);
    assert_eq!(world.preferences().window.mode, 2);
    assert_eq!(
        world.window.calls()[1..],
        [WindowCall::Windowed, WindowCall::Resizable(true)]
    );
    Ok(())
}

/// One binding of the Controls tab per family: `(family, button, key)`. The
/// button is the row's "Change Keybind" component (its label, the key's name,
/// is the next component), the key an AWT code no default uses.
const FAMILIES: &[(&str, ComponentId, i32)] = &[
    ("camera", controls_settings::CAMERA_UP_KEYBIND, 89),
    (
        "windows and navigation",
        controls_settings::WINDOWS_NAVIGATION_KEYBIND,
        74,
    ),
    (
        "interface tabs",
        controls_settings::INTERFACE_TABS_KEYBIND,
        86,
    ),
    ("chat", controls_settings::CHAT_KEYBIND, 90),
    ("action bar", controls_settings::ACTION_BAR_KEYBIND, 88),
    ("combat", controls_settings::COMBAT_KEYBIND, 67),
    ("audio", controls_settings::MUSIC_MUTE_KEYBIND, 77),
    ("toggle run", controls_settings::TOGGLE_RUN_KEYBIND, 75),
    ("world map", controls_settings::WORLD_MAP_KEYBIND, 79),
    (
        "lock interface",
        controls_settings::LOCK_INTERFACE_KEYBIND,
        85,
    ),
    (
        "home teleport",
        controls_settings::HOME_TELEPORT_KEYBIND,
        73,
    ),
    ("global mute", controls_settings::GLOBAL_MUTE_KEYBIND, 78),
];

/// The label beside a "Change Keybind" button: the key's name.
fn key_label(button: ComponentId) -> ComponentId {
    ComponentId(button.0 + 1)
}

fn key_name(awt: i32) -> String {
    char::from_u32(awt as u32).unwrap().to_string()
}

fn rebind(world: &mut SettingsWorld, button: ComponentId, key: i32) -> anyhow::Result<()> {
    world.op(button, -1)?;
    world.press(key, 4)?;
    world.ticks(4)
}

/// Every family rebinds through the Controls tab: the row shows the new key,
/// the binding survives a new login, and Reset All Keybinds brings every row
/// back to the text it first showed.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn controls_every_family_rebinds_persists_and_resets() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("families", &[[1920, 1200]])?;
    world.open_tab(3)?;
    let mut defaults = vec![];
    for &(_, button, _) in FAMILIES {
        defaults.push(world.text(key_label(button))?);
    }
    for &(family, button, key) in FAMILIES {
        world.ui.diagnostics.errors.clear();
        rebind(&mut world, button, key)?;
        assert!(
            world.ui.diagnostics.errors.is_empty(),
            "{family}: {:?}",
            world.ui.diagnostics.errors
        );
        assert_eq!(world.text(key_label(button))?, key_name(key), "{family}");
    }
    // A new login of the same account shows and keeps them all.
    world.relogin()?;
    world.open_tab(3)?;
    for &(family, button, key) in FAMILIES {
        assert_eq!(
            world.text(key_label(button))?,
            key_name(key),
            "{family} after login"
        );
    }
    // Reset All Keybinds: the button opens the confirmation, which runs the
    // per-family reset timers to the end.
    world.op(controls_settings::RESET_KEYBINDS_BUTTON, -1)?;
    assert!(
        world.is_open(interface::CONFIRM_POPUP),
        "the reset confirmation is up"
    );
    world.op(confirm_popup::CONFIRM_BUTTON, -1)?;
    world.ticks(80)?;
    assert!(!world.is_open(interface::CONFIRM_POPUP));
    for (&(family, button, _), default) in FAMILIES.iter().zip(&defaults) {
        assert_eq!(
            &world.text(key_label(button))?,
            default,
            "{family} after reset"
        );
    }
    Ok(())
}

/// The families whose consumers the dev world provides act on the rebound
/// key at once, and their old key does nothing any more.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn rebound_keys_reach_their_consumers() -> anyhow::Result<()> {
    // (family, button, old key, new key)
    let live: &[(&str, ComponentId, i32, i32)] = &[
        ("toggle run", controls_settings::TOGGLE_RUN_KEYBIND, 82, 75),
        // The open map closes on its own M key, so only the new key is checked.
        ("world map", controls_settings::WORLD_MAP_KEYBIND, 0, 74),
        (
            "lock interface",
            controls_settings::LOCK_INTERFACE_KEYBIND,
            76,
            85,
        ),
        (
            "home teleport",
            controls_settings::HOME_TELEPORT_KEYBIND,
            84,
            73,
        ),
        (
            "global mute",
            controls_settings::GLOBAL_MUTE_KEYBIND,
            71,
            78,
        ),
        ("music mute", controls_settings::MUSIC_MUTE_KEYBIND, 0, 77),
    ];
    let mut world = SettingsWorld::login("consumers", &[[1920, 1200]])?;
    world.open_tab(3)?;
    for &(_, button, _, key) in live {
        rebind(&mut world, button, key)?;
    }
    world.op(game_window::MODAL_CLOSE_BUTTON, 1)?;
    world.ticks(10)?;
    // What a key sends beyond its own key-press report (opcode 87, 7 bytes).
    let sent = |world: &mut SettingsWorld, key: i32| -> anyhow::Result<Vec<u8>> {
        world.ui.engine.outgoing.clear();
        world.press(key, 6)?;
        let out = world.ui.engine.outgoing.clone();
        let mut rest = vec![];
        let mut at = 0;
        while at < out.len() {
            if out[at] == 87 && at + 7 <= out.len() {
                at += 7;
            } else {
                rest.push(out[at]);
                at += 1;
            }
        }
        Ok(rest)
    };
    for &(family, _, old, new) in live {
        let action = sent(&mut world, new)?;
        assert_eq!(action.len(), 9, "{family}: the new key sent {action:?}");
        if old != 0 {
            assert!(
                sent(&mut world, old)?.is_empty(),
                "{family}: the old key still acts"
            );
        }
    }
    Ok(())
}

/// The Audio tab: each slider sets its channel's volume through the cache
/// script into the saved options the audio owner reads every cycle, and the
/// mute toggles go to the server, whose published mute bits silence and
/// restore the channels.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn audio_sliders_and_mutes_reach_the_mixer() -> anyhow::Result<()> {
    // (slider track, option, the channel's field in the audio owner's view)
    type Channel = fn(&crate::audio_backend::VolumePreferences) -> i32;
    let sliders: &[(ComponentId, &str, Channel)] = &[
        (audio_settings::MUSIC_VOLUME_TRACK, "unknownVolume1", |v| {
            v.music
        }),
        (
            audio_settings::SOUND_EFFECTS_VOLUME_TRACK,
            "soundVolume",
            |v| v.sound,
        ),
        (
            audio_settings::AMBIENT_VOLUME_TRACK,
            "backgroundSoundVolume",
            |v| v.background_sound,
        ),
        (audio_settings::VOICE_VOLUME_TRACK, "speechVolume", |v| {
            v.speech
        }),
    ];
    let mut world = SettingsWorld::login("audio", &[[1920, 1200]])?;
    world.open_tab(4)?;
    for &(track, option, channel) in sliders {
        let before = field(option, &world);
        world.click(track)?;
        let after = field(option, &world);
        assert_ne!(before, after, "slider {track:?} did not move {option}");
        assert_eq!(
            channel(&world.ui.audio.volumes()),
            after,
            "{option} did not reach the mixer"
        );
        assert_persisted(&mut world, option)?;
    }
    // A mute toggle is a server operation: the client sends the button press.
    world.ui.engine.outgoing.clear();
    let toggle = audio_settings::MASTER_MUTE_LABEL;
    world.op(toggle, -1)?;
    let sent = world.ui.engine.outgoing.clone();
    assert_eq!(sent.len(), 9, "the toggle sends one button packet");
    assert_eq!(&sent[5..9], &toggle.0.to_be_bytes());
    let volumes = |w: &SettingsWorld| {
        let v = w.ui.audio.volumes();
        [v.music, v.sound, v.background_sound, v.speech]
    };
    let playing = volumes(&world);
    assert!(playing.iter().all(|&v| v > 0));
    // The server's answer, as its varp update reaches the client: bit 0 is
    // the global mute, bits 1-4 mute music, effects, ambience and voice.
    let publish = |w: &mut SettingsWorld, bits: i32| -> anyhow::Result<()> {
        let (clock, game) = (&w.clock, &mut w.game);
        game.install_server_varp(varp::AUDIO_MUTES.id(), bits, clock.0)
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        w.ticks(10)
    };
    publish(&mut world, 0b00001)?;
    assert_eq!(volumes(&world), [0, 0, 0, 0], "global mute");
    publish(&mut world, 0)?;
    assert_eq!(volumes(&world), playing, "unmuting restores the volumes");
    for (bit, channel) in [(0b00010, 0), (0b00100, 1), (0b01000, 2), (0b10000, 3)] {
        publish(&mut world, bit)?;
        let now = volumes(&world);
        for (index, (&level, &was)) in now.iter().zip(&playing).enumerate() {
            assert_eq!(
                level,
                if index == channel { 0 } else { was },
                "mute bit {bit:#b}"
            );
        }
    }
    publish(&mut world, 0)?;
    Ok(())
}

/// The brightness slider changes the light the environment and the scene
/// build use, and rebuilds the scene.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn graphics_brightness_slider_changes_the_light() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("brightness", &[[1920, 1200]])?;
    world.open_tab(2)?;
    let start = seen(&world);
    // The two ends of the slider track.
    let mut levels = vec![start.live.brightness];
    for track in [panel::BRIGHTNESS_TRACK_LEFT, panel::BRIGHTNESS_TRACK_RIGHT] {
        world.click(track)?;
        let now = seen(&world);
        assert_eq!(now.live.brightness, now.build.brightness);
        assert_persisted(&mut world, "brightness")?;
        levels.push(now.live.brightness);
    }
    assert!(
        levels.windows(2).any(|w| w[0] != w[1]),
        "brightness never moved: {levels:?}"
    );
    assert!(levels.iter().all(|b| (1..=4).contains(b)), "{levels:?}");
    assert!(has_effect(&world, PreferenceEffect::SceneRebuild));
    Ok(())
}
