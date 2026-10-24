//! The preference-changing CS2 commands. Mutations with installed runtime
//! consumers live here alongside trivial save-only preference mutations
//! (particles, skyboxes, build area, loading screen, stereo). Graphics presets
//! (`autosetup_sethigh/setmedium/setlow/setmin`) include the toolkit bloom
//! disable, model-cache, environment-fade and canvas-changed tails.
//! `detail_toolkit` and `detail_antialiasing` recreate the toolkit
//! (`ui_preferences_toolkit.rs`); `autosetup_dosetup` runs the auto-setup
//! probe (`ui_preferences_autosetup.rs`).
use super::{pop, Preferences};
use native910::vm::{Value, VmResult};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreferenceEffect {
    SceneRebuild,
    ResetModelCaches,
    SetupRoofs,
    /// A toolkit change: delete and recreate the device, rebind input, reset
    /// the cursor and the model caches.
    RecreateToolkit,
    /// Restart the environment's fade between lighting states.
    ResetEnvironmentFade,
}

/// The named graphics quality presets. The preset id is what the `preset`
/// option stores and what the auto-setup reports (0 is a custom setup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityLevel {
    Minimum,
    Low,
    Medium,
    High,
}
impl QualityLevel {
    /// The preset id stored in the options.
    pub const fn preset_id(self) -> i32 {
        match self {
            QualityLevel::Minimum => 1,
            QualityLevel::Low => 2,
            QualityLevel::Medium => 3,
            QualityLevel::High => 4,
        }
    }
    /// The level an `autosetup_set*` command selects.
    fn from_command(command: &str) -> Option<Self> {
        match command {
            "autosetup_sethigh" => Some(QualityLevel::High),
            "autosetup_setmedium" => Some(QualityLevel::Medium),
            "autosetup_setlow" => Some(QualityLevel::Low),
            "autosetup_setmin" => Some(QualityLevel::Minimum),
            _ => None,
        }
    }
}

#[cfg(test)]
#[path = "bloom_settings_tests.rs"]
mod bloom_settings_tests;
impl Preferences {
    pub fn drain_effects(&mut self) -> Vec<PreferenceEffect> {
        std::mem::take(&mut self.pending_effects)
    }
    pub fn mutate_graphics(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        profiler: Option<&mut super::Profiler<'_>>,
    ) -> Option<VmResult<Option<Value>>> {
        use PreferenceEffect::*;
        if command == "autosetup_dosetup" {
            return Some({
                let result = self.get_autosetup_result(profiler);
                let display_mode = self.options.get("displayMode").unwrap();
                self.autosetup_display_mode = display_mode;
                ints.extend([display_mode, result]);
                // Reset the model caches, rebuild the scene and save.
                self.pending_effects.push(ResetModelCaches);
                self.pending_effects.push(SceneRebuild);
                self.dirty = true;
                self.change_notified = false;
                Ok(None)
            });
        }
        if command == "profile_cpu" {
            return Some(Ok(Some(Value::Int(super::cpu_profile()))));
        }
        if command == "detailget_performance_metric" {
            let toolkit = self.options.get("displayMode").unwrap();
            return Some(Ok(Some(Value::Int(
                self.performance_metric(toolkit, 200, profiler),
            ))));
        }
        if command == "detail_toolkit" {
            // An unknown or out-of-range toolkit id selects toolkit 3.
            return Some((|| {
                let mut toolkit = pop(ints)?;
                if !(0..=5).contains(&toolkit) || toolkit == 2 {
                    toolkit = 3;
                }
                self.set_toolkit(toolkit, false);
                Ok(None)
            })());
        }
        if command == "detail_antialiasing" {
            // Changing the sample level recreates the toolkit.
            return Some((|| {
                let value = pop(ints)?;
                self.options.set_field("antiAliasing2", value).unwrap();
                let toolkit = self.options.get("displayMode").unwrap();
                self.set_toolkit(toolkit, false);
                self.dirty = true;
                Ok(None)
            })());
        }
        if command == "autosetup_dosetupstatus" {
            // The status reports two zero flags until the native auto-setup
            // result service is installed.
            ints.extend([0, 0]);
            return Some(Ok(None));
        }
        if let Some(quality) = QualityLevel::from_command(command) {
            return Some({
                // Apply the preset, reset the model caches, rebuild the scene
                // and save.
                self.apply_preset_level(quality.preset_id());
                self.pending_effects.push(SceneRebuild);
                self.dirty = true;
                self.change_notified = false;
                Ok(None)
            });
        }
        if command == "detail_toolkit_default" {
            return Some((|| {
                if ints.len() < 2 {
                    return Err(native910::vm::VmError::StackUnderflow { stack: "int" });
                }
                let keep_preset = pop(ints)? == 1;
                let toolkit = pop(ints)?;
                self.options.set_field("toolkit", toolkit).unwrap();
                if !keep_preset {
                    self.options.set_field("preset", 0).unwrap();
                }
                self.dirty = true;
                self.change_notified = false;
                Ok(None)
            })());
        }
        if command == "detail_bloom" {
            // Anything but 0 or 1 turns bloom off.
            return Some((|| {
                let mut value = pop(ints)?;
                if !(0..=1).contains(&value) {
                    value = 0;
                }
                self.set_bloom(value == 1);
                Ok(None)
            })());
        }
        if command == "detail_waterdetail_high" {
            return Some((|| {
                // `waterDetail = pop == 1 ? 2 : 0`, rebuild the scene, save.
                // The rebuild consumer is `rebuild::BuildPrefs::water_detail`
                // (underwater floor and water-detail floors).
                let high = pop(ints)? == 1;
                self.options
                    .set_field("waterDetail", if high { 2 } else { 0 })
                    .unwrap();
                self.pending_effects.push(SceneRebuild);
                self.dirty = true;
                self.change_notified = false;
                Ok(None)
            })());
        }
        if command == "detail_loadingscreentype" {
            return Some((|| {
                // Clamp to 0-255 (out of range becomes 0), then change-check
                // before saving.
                let mut value = pop(ints)?;
                if !(0..=255).contains(&value) {
                    value = 0;
                }
                if self.options.get("loadingScreen") == Some(value) {
                    return Ok(None);
                }
                self.options.set_field("loadingScreen", value).unwrap();
                self.dirty = true;
                self.change_notified = false;
                Ok(None)
            })());
        }
        let (field, conversion, effect, save) = match command {
            "detail_brightness" => ("brightness", 0, Some(SceneRebuild), true),
            "detail_particles" => ("particles", 0, None, true),
            "detail_skydetail" => ("skyboxes", 0, None, true),
            "detail_buildarea" => ("buildArea", 0, None, true),
            "detail_stereo" => ("stereo", 1, None, true),
            "detail_grounddecor_on" => ("groundDecoration", 1, Some(SceneRebuild), true),
            "detail_idleanims_many" | "detail_idleanims" => ("idleAnimations", 0, None, true),
            "detail_flickering_on" => ("flickeringEffects", 1, None, true),
            "detail_customcursors" => ("customCursors", 2, None, true),
            "detail_cpuusage" => ("cpuUsage", 0, None, true),
            "detail_antialiasing_default" => ("antiAliasing", 0, None, true),
            "detail_spotshadows_on" => ("characterShadows", 1, None, true),
            "detail_hardshadows" => ("sceneryShadows", 0, Some(SceneRebuild), true),
            "detail_shadowquality" => ("shadowQuality", 0, Some(SceneRebuild), true),
            "detail_lightdetail_high" => ("lightingDetail", 1, Some(ResetModelCaches), true),
            "detail_fog_on" => ("fog", 1, Some(SceneRebuild), true),
            "detail_groundblending" => ("groundBlending", 2, Some(SceneRebuild), true),
            "detail_texturing" => ("textures", 1, Some(ResetModelCaches), true),
            "detail_maxscreensize" => ("screenSize", 0, None, true),
            "detail_animdetail" => ("animDetail", 0, Some(SceneRebuild), true),
            "detail_removeroofs_option" => ("removeRoofs", 0, Some(SetupRoofs), true),
            "detail_removeroofs_option_override" => ("removeRoofs2", 0, Some(SetupRoofs), false),
            "autosetup_setcustom" => ("preset", 3, None, true),
            _ => return None,
        };
        Some((|| {
            let raw = if conversion == 3 { 0 } else { pop(ints)? };
            if crate::toolkit_debug_flags::flags().settings_trace {
                log::info!("[settings] {command} {raw}");
            }
            if matches!(command, "detail_animdetail" | "detail_skydetail")
                && self.options.get(field) == Some(raw)
            {
                return Ok(None);
            }
            let value = match conversion {
                1 => i32::from(raw == 1),
                2 => i32::from(raw != 0),
                _ => raw,
            };
            let value = if command == "detail_removeroofs_option_override" && value == -1 {
                self.options.get("removeRoofs").unwrap()
            } else {
                value
            };
            self.options.set_field(field, value).unwrap();
            if command == "detail_removeroofs_option" {
                self.options.set_field("removeRoofs2", value).unwrap();
            }
            if let Some(effect) = effect {
                self.pending_effects.push(effect);
            }
            if save {
                self.dirty = true;
            }
            // The saving detail commands also mark the preferences as
            // unreported.
            if matches!(
                command,
                "detail_brightness"
                    | "detail_removeroofs_option"
                    | "detail_grounddecor_on"
                    | "detail_idleanims_many"
                    | "detail_flickering_on"
                    | "detail_spotshadows_on"
                    | "detail_hardshadows"
                    | "detail_shadowquality"
                    | "detail_lightdetail_high"
                    | "detail_fog_on"
                    | "detail_stereo"
                    | "detail_particles"
                    | "detail_buildarea"
                    | "detail_texturing"
                    | "detail_skydetail"
                    | "detail_animdetail"
                    | "autosetup_setcustom"
            ) {
                self.change_notified = false;
            }
            if command == "detail_maxscreensize" {
                // The canvas size may have changed.
                self.window.changed = true;
            }
            if command == "detail_lightdetail_high" {
                // A lighting change restarts the environment fade.
                self.pending_effects.push(ResetEnvironmentFade);
            }
            Ok(None)
        })())
    }
}

#[cfg(test)]
#[test]
fn graphics_commands_match_the_recording() -> anyhow::Result<()> {
    let input = rs910_core::test_support::frozen::text("graphics-settings/input.txt");
    let expected = rs910_core::test_support::frozen::text("graphics-settings/expected.txt");
    let mut actual = Vec::new();
    let mut fields = crate::client_options::FIELDS.to_vec();
    fields.sort();
    for line in input.lines() {
        let mut p = Preferences {
            options: crate::client_options::ClientOptions::new(crate::client_options::Profile {
                arm: false,
                windows: false,
                initial_display_mode: 5,
                ..Default::default()
            }),
            ..Default::default()
        };
        let a: Vec<_> = line.split_whitespace().collect();
        let mut ints = if a[0] == "autosetup_setcustom" {
            vec![]
        } else {
            vec![a[1].parse()?]
        };
        p.dispatch(a[0], &mut ints)
            .ok_or_else(|| anyhow::anyhow!("unhandled {}", a[0]))?
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        let (mut rebuild, mut roofs, mut models) = (0, 0, 0);
        for effect in p.drain_effects() {
            match effect {
                PreferenceEffect::SceneRebuild => rebuild += 1,
                PreferenceEffect::ResetModelCaches => {
                    rebuild += 1;
                    models += 1;
                }
                PreferenceEffect::SetupRoofs => roofs += 1,
                // A toolkit change resets the model caches.
                PreferenceEffect::RecreateToolkit => {
                    rebuild += 1;
                    models += 1;
                }
                PreferenceEffect::ResetEnvironmentFade => {}
            }
        }
        let values = fields
            .iter()
            .map(|f| format!("{f}={}", p.options.get(f).unwrap()))
            .collect::<Vec<_>>()
            .join(",");
        let encoded = p
            .options
            .encode()
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect::<String>();
        actual.push(format!(
            "{values}|{encoded}|{},{rebuild},{roofs},{models}",
            i32::from(p.dirty)
        ));
    }
    let expected: Vec<_> = expected.lines().collect();
    assert_eq!(actual.len(), expected.len());
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(a, e, "{}", input.lines().nth(i).unwrap());
    }
    Ok(())
}
