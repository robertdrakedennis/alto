//! The graphics panel's auto-setup control, pressed through the real
//! interface: the cache script runs `autosetup_dosetup`, which profiles each
//! hardware toolkit on the renderer lent to the cycle (a scripted one here,
//! so the timings are injected), shows the "Profiling..." box before every
//! benchmark, and applies the preset and toolkit the measurements select.
use super::settings_world::*;
use crate::ui_preferences::metric::Backdrop;
use crate::ui_preferences::{AutoSetupResult, MetricContext, PreferenceEffect, ProbeSlot};
use rs910_symbols::component::graphics_settings_panel::{AUTO_SETUP_BUTTON, CONTROL_LIST};
use rs910_symbols::interface;

/// Where the Yes button of the timed "Accept this setting?" box
/// (`graphics_change_confirm`) is on the test canvas (1280x720).
const KEEP_SETTING_YES: [i32; 2] = [574, 467];

/// The first-run "Updating graphics settings" box and the keep-setting box.
fn assert_no_dialog(world: &SettingsWorld) {
    let subs = world.subs();
    for dialog in [
        interface::GRAPHICS_UPDATE_NOTICE.id(),
        interface::GRAPHICS_CHANGE_CONFIRM.id(),
    ] {
        assert!(
            !subs.contains(&dialog),
            "dialog {dialog} is left open: {subs:?}"
        );
    }
}

/// A logged-in world on the graphics tab, on a machine that is not Windows,
/// running the GL toolkit, with the benchmark model's cache defaults.
fn graphics_world(name: &str) -> anyhow::Result<SettingsWorld> {
    let mut world =
        SettingsWorld::login(name, &[[1920, 1200], [1280, 720], [1024, 768], [800, 600]])?;
    world.open_tab(2)?;
    let pack_root = world.pack.root().to_path_buf();
    let preferences = world.preferences_mut();
    preferences.options.profile.windows = false;
    preferences.options.set_field("displayMode", 5).unwrap();
    preferences.performance_metrics_model = Some(47000);
    preferences.metric_context = Some(MetricContext {
        pack_root,
        map_size_x: 0,
    });
    preferences.pending_effects.clear();
    preferences.graphics_packets.clear();
    Ok(world)
}

fn report(world: &SettingsWorld) -> Vec<u8> {
    world
        .preferences()
        .graphics_packets
        .iter()
        .find(|packet| packet[0] == crate::proto::client::AUTO_SETUP_RESULT)
        .expect("the auto-setup report is queued for the server")
        .clone()
}

/// Both toolkits are profiled, each behind its own "Profiling..." box (the
/// second one after a toolkit switch, on a cleared canvas), the one the
/// timings favour is chosen, its metric picks the preset, and the report
/// that goes to the server carries the measurements.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_auto_setup_control_profiles_the_toolkits_and_applies_the_recommendation(
) -> anyhow::Result<()> {
    let mut world = graphics_world("profiling-measured")?;
    // Toolkit 5 draws 60000 models a second, toolkit 1 30000.
    world.probe.rates = [60_000, 30_000].into();
    world.op(AUTO_SETUP_BUTTON, -1)?;
    let box_over = |backdrop| ProbeEvent::Box {
        backdrop,
        drawn: true,
    };
    let bench = ProbeEvent::Benchmark {
        budget_ms: 1000,
        canvas: [1280, 720],
    };
    assert_eq!(
        world.probe.events,
        [
            box_over(Backdrop::LastFrame),
            bench.clone(),
            box_over(Backdrop::Black),
            bench
        ]
    );
    // Toolkit 1's 30000 is the better weighted result (DirectX is not
    // offered), and above 10000 it is the low preset.
    let options = world.options();
    assert_eq!(options.get("displayMode"), Some(1));
    assert_eq!(options.get("toolkit"), Some(1));
    assert_eq!(options.get("preset"), Some(2));
    assert_eq!(options.get("safeMode"), Some(0));
    let mut expected = AutoSetupResult::default();
    expected.add_flags(AutoSetupResult::FLAG_TOOLKIT5_OK | AutoSetupResult::FLAG_TOOLKIT1_OK);
    expected.record_metric(ProbeSlot::Toolkit5, 60_000);
    expected.record_metric(ProbeSlot::Toolkit1, 30_000);
    expected.set_chosen_toolkit(1);
    expected.result = 2;
    assert_eq!(report(&world), expected.encode());
    let effects = &world.preferences().pending_effects;
    for effect in [
        PreferenceEffect::RecreateToolkit,
        PreferenceEffect::ResetModelCaches,
        PreferenceEffect::SceneRebuild,
    ] {
        assert!(
            effects.contains(&effect),
            "{effect:?} missing from {effects:?}"
        );
    }
    assert!(
        world.ui.diagnostics.errors.is_empty(),
        "{:?}",
        world.ui.diagnostics.errors
    );
    // The toolkit changed, so the interface asks to keep the setting (the
    // timed "Accept this setting?" box); accepting it leaves nothing open.
    assert!(
        world.is_open(interface::GRAPHICS_CHANGE_CONFIRM),
        "{:?}",
        world.subs()
    );
    world.click_at(KEEP_SETTING_YES)?;
    assert_no_dialog(&world);
    assert_eq!(world.options().get("displayMode"), Some(1));
    Ok(())
}

/// A renderer that cannot benchmark scores both toolkits -1: the setup falls
/// back to the software toolkit, preset from the CPU probe.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_auto_setup_control_falls_back_to_the_software_toolkit() -> anyhow::Result<()> {
    let mut world = graphics_world("profiling-unmeasured")?;
    world.op(AUTO_SETUP_BUTTON, -1)?;
    let benchmarks = world
        .probe
        .events
        .iter()
        .filter(|event| matches!(event, ProbeEvent::Benchmark { .. }))
        .count();
    assert_eq!(benchmarks, 2);
    let options = world.options();
    assert_eq!(options.get("displayMode"), Some(0));
    assert_eq!(options.get("toolkit"), Some(0));
    assert!((1..=4).contains(&options.get("preset").unwrap()));
    let packet = report(&world);
    // Chosen toolkit 0 (carried plus 128), and no toolkit's metric.
    assert_eq!(packet[3], 128);
    Ok(())
}

/// Settings, then the Graphics tab, through the interface host: the tab's
/// panel is populated (the control rows and the auto-setup button) and no dialog is left open on the way. The panel is
/// the one a player sees before pressing anything, so a script that waits
/// for a result the client never delivers (the profiling it starts, a
/// dialog's resume) leaves it empty and this fails.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn the_graphics_tab_is_populated_and_leaves_no_dialog_open() -> anyhow::Result<()> {
    let mut world = SettingsWorld::login("profiling-panel", &[[1920, 1200]])?;
    assert_no_dialog(&world);
    world.open_tab(2)?;
    let subs = world.subs();
    assert!(
        subs.contains(&interface::GRAPHICS_SETTINGS.id())
            && subs.contains(&interface::GRAPHICS_SETTINGS_PANEL.id()),
        "{subs:?}"
    );
    assert_no_dialog(&world);
    let rows = world
        .ui
        .store
        .get(CONTROL_LIST.packed(), -1)?
        .map_or(0, |panel| {
            panel
                .borrow()
                .children
                .as_ref()
                .map_or(0, |rows| rows.borrow().len())
        });
    assert!(rows >= 19, "the control rows number {rows}");
    assert!(world
        .ui
        .store
        .get(AUTO_SETUP_BUTTON.packed(), -1)?
        .is_some());
    assert!(
        world.ui.diagnostics.errors.is_empty(),
        "{:?}",
        world.ui.diagnostics.errors
    );
    Ok(())
}
