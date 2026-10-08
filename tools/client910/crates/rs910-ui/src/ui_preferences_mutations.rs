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
mod tests {
    use super::*;
    #[test]
    fn water_detail_high_sets_two_and_rebuilds() {
        let mut p = Preferences::default();
        p.dispatch("detail_waterdetail_high", &mut vec![1])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("waterDetail"), Some(2));
        assert!(p.drain_effects().contains(&PreferenceEffect::SceneRebuild));
        p.dispatch("detail_waterdetail_high", &mut vec![0])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("waterDetail"), Some(0));
    }

    #[test]
    fn checkbox_conversion_and_roof_override() {
        let mut p = Preferences::default();
        p.dispatch("detail_grounddecor_on", &mut vec![2])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("groundDecoration"), Some(0));
        assert_eq!(p.drain_effects(), [PreferenceEffect::SceneRebuild]);
        p.dispatch("detail_groundblending", &mut vec![2])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("groundBlending"), Some(1));
        assert_eq!(p.drain_effects(), [PreferenceEffect::SceneRebuild]);
        p.dispatch("detail_shadowquality", &mut vec![3])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("shadowQuality"), Some(3));
        assert_eq!(p.drain_effects(), [PreferenceEffect::SceneRebuild]);
        p.dispatch("detail_removeroofs_option", &mut vec![1])
            .unwrap()
            .unwrap();
        p.dirty = false;
        p.dispatch("detail_removeroofs_option_override", &mut vec![0])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("removeRoofs"), Some(1));
        assert_eq!(p.options.get("removeRoofs2"), Some(0));
        assert!(!p.dirty);
        p.dispatch("detail_removeroofs_option_override", &mut vec![-1])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("removeRoofs2"), Some(1));
    }
    #[test]
    fn detail_toolkit_does_not_save_the_default_toolkit() {
        // `detail_toolkit` changes only the active toolkit (`displayMode`)
        // through the toolkit switch; the saved
        // `toolkit` slot belongs to `detail_toolkit_default`.
        let mut p = Preferences::default();
        let toolkit = p.options.get("toolkit");
        p.dispatch("detail_toolkit", &mut vec![1]).unwrap().unwrap();
        assert_eq!(p.options.get("displayMode"), Some(1));
        assert_eq!(p.options.get("toolkit"), toolkit);
        assert!(!p.dirty);
        assert!(p
            .drain_effects()
            .contains(&PreferenceEffect::RecreateToolkit));
    }
    #[test]
    fn autosetup_presets_apply_preference_state_only() {
        // Dispatch shape: preset field values, dirty save flag, and the
        // queued ResetModelCaches + SceneRebuild effects (real consumers
        // partial). Default profile is dual-CPU (cpu_count 2) so cpuUsage
        // ends at 4.
        for (command, preset) in [
            ("autosetup_setmin", 1),
            ("autosetup_setlow", 2),
            ("autosetup_setmedium", 3),
            ("autosetup_sethigh", 4),
        ] {
            let mut p = Preferences {
                dirty: false,
                ..Default::default()
            };
            p.dispatch(command, &mut vec![]).unwrap().unwrap();
            assert_eq!(p.options.get("preset"), Some(preset), "{command}");
            assert_eq!(p.options.get("cpuUsage"), Some(4), "{command}");
            assert!(p.dirty, "{command}");
            assert_eq!(
                p.drain_effects(),
                vec![
                    PreferenceEffect::ResetModelCaches,
                    PreferenceEffect::ResetEnvironmentFade,
                    PreferenceEffect::SceneRebuild
                ],
                "{command}"
            );
            // v38 state round-trips byte-for-byte.
            let bytes = p.options.encode();
            assert_eq!(bytes.len(), 58);
            assert_eq!(
                crate::client_options::ClientOptions::decode(&bytes, p.options.profile)
                    .unwrap()
                    .encode(),
                bytes,
                "{command}"
            );
        }
        // Spot-check divergent fields across the four levels.
        let mut p = Preferences::default();
        p.dispatch("autosetup_sethigh", &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("sceneryShadows"), Some(2));
        assert_eq!(p.options.get("waterDetail"), Some(2));
        assert_eq!(p.options.get("particles"), Some(2));
        assert_eq!(p.options.get("screenSize"), Some(0));
        assert_eq!(p.options.get("skyboxes"), Some(1));
        assert_eq!(p.options.get("animDetail"), Some(1));
        p.dispatch("autosetup_setmedium", &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("sceneryShadows"), Some(1));
        assert_eq!(p.options.get("waterDetail"), Some(0));
        assert_eq!(p.options.get("particles"), Some(1));
        assert_eq!(p.options.get("screenSize"), Some(1));
        p.dispatch("autosetup_setlow", &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("groundDecoration"), Some(1));
        assert_eq!(p.options.get("idleAnimations"), Some(0));
        assert_eq!(p.options.get("textures"), Some(0));
        assert_eq!(p.options.get("preset"), Some(2));
        p.dispatch("autosetup_setmin", &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("groundDecoration"), Some(0));
        assert_eq!(p.options.get("groundBlending"), Some(0));
        assert_eq!(p.options.get("fog"), Some(0));
        assert_eq!(p.options.get("preset"), Some(1));
        // `detail_toolkit_default` stores the
        // saved slot and clears the preset unless kept; no device change.
        let mut p = Preferences {
            dirty: false,
            ..Default::default()
        };
        p.dispatch("detail_toolkit_default", &mut vec![1, 0])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("toolkit"), Some(1));
        assert_eq!(p.options.get("preset"), Some(0));
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
    }

    #[test]
    fn save_only_preference_mutations() {
        // `detail_particles`: unconditional set + save, with the
        // particle_level side mirror for out-of-range values.
        let mut p = Preferences {
            dirty: false,
            ..Default::default()
        };
        p.dispatch("detail_particles", &mut vec![2])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("particles"), Some(2));
        assert_eq!(p.options.particle_level, 2);
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
        p.dirty = false;
        p.dispatch("detail_particles", &mut vec![9])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("particles"), Some(2));
        assert_eq!(p.options.particle_level, 0);
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
        // `detail_skydetail`: change-checked set + save, no rebuild.
        let mut p = Preferences::default();
        let current = p.options.get("skyboxes").unwrap();
        p.dirty = false;
        p.dispatch("detail_skydetail", &mut vec![current])
            .unwrap()
            .unwrap();
        assert!(!p.dirty);
        assert!(p.drain_effects().is_empty());
        let flipped = if current == 1 { 0 } else { 1 };
        p.dispatch("detail_skydetail", &mut vec![flipped])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("skyboxes"), Some(flipped));
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
        // `detail_buildarea`: unconditional set + save.
        let mut p = Preferences {
            dirty: false,
            ..Default::default()
        };
        p.dispatch("detail_buildarea", &mut vec![2])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("buildArea"), Some(2));
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
        // `detail_loadingscreentype`: 0-255 clamp (else 0),
        // change-checked before saving.
        let mut p = Preferences::default();
        assert_eq!(p.options.get("loadingScreen"), Some(0));
        p.dirty = false;
        p.dispatch("detail_loadingscreentype", &mut vec![5])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("loadingScreen"), Some(5));
        assert!(p.dirty);
        p.dirty = false;
        p.dispatch("detail_loadingscreentype", &mut vec![300])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("loadingScreen"), Some(0));
        assert!(p.dirty);
        p.dirty = false;
        p.dispatch("detail_loadingscreentype", &mut vec![-1])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("loadingScreen"), Some(0));
        assert!(!p.dirty);
        assert!(p.drain_effects().is_empty());
        // `detail_stereo`: boolean conversion, unconditional save.
        let mut p = Preferences {
            dirty: false,
            ..Default::default()
        };
        p.dispatch("detail_stereo", &mut vec![1]).unwrap().unwrap();
        assert_eq!(p.options.get("stereo"), Some(1));
        assert!(p.dirty);
        p.dirty = false;
        p.dispatch("detail_stereo", &mut vec![7]).unwrap().unwrap();
        assert_eq!(p.options.get("stereo"), Some(0));
        assert!(p.dirty);
        assert!(p.drain_effects().is_empty());
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
