//! The toolkit lifecycle on the preference owner: switching and creating a
//! toolkit, bloom, the startup toolkit selection, the safe-mode recovery at
//! startup and its confirmation in the main loop, and the queue of graphics
//! reports for the server.
//!
//! The process owns one native GPU device (the `render::Renderer`). It hosts
//! the hardware toolkit ids 1 (GL) and 5 (shader GL): both are accelerated
//! toolkits, which is the only toolkit distinction the preference rules
//! observe. Toolkit 0 (the software toolkit) is game state: it answers no
//! bloom and no anti-aliasing, and the GPU renderer draws it.
//!
//! Toolkit 3 (DirectX) is unavailable by design, on every operating system:
//! the GL toolkit is the hardware toolkit (wgpu runs it on Metal, Vulkan or
//! DirectX 12). The client therefore behaves as the original does on a
//! machine without the DirectX library. The option rules refuse toolkit 3
//! (`detailcanset_toolkit_default(3)` answers 3), the default toolkit is GL,
//! the auto-setup does not probe DirectX, and a request for toolkit 3 (the
//! settings panel, `detail_toolkit`, a saved file) fails in
//! [`Preferences::create_toolkit`]: the failure is reported to the server
//! (type 4) and the toolkit that was active is created again. At startup the
//! loading-stage software toolkit is the one that was active, so a saved
//! toolkit 3 starts on the software toolkit and stays saved as 3.
//! A device change is queued as [`PreferenceEffect::RecreateToolkit`]; the app
//! consumer drops and recreates the surface/scene targets and model caches
//! and selects toolkit 0's answers for `displayMode == 0`.
use super::{PreferenceEffect, Preferences};

/// The toolkit-change report ids sent to the server.
pub mod toolkit_type {
    /// GL toolkit creation failed.
    pub const GL_FAILED: i32 = 3;
    /// DirectX toolkit creation failed.
    pub const DX_FAILED: i32 = 4;
    /// Safe mode started without an explicit choice.
    pub const SAFE_MODE: i32 = 9;
    /// The saved toolkit is the software renderer.
    pub const SOFTWARE_SAVED: i32 = 10;
    /// A GL crash was detected at startup.
    pub const GL_CRASHED: i32 = 12;
    /// A DirectX crash was detected at startup.
    pub const DX_CRASHED: i32 = 13;
}

impl Preferences {
    /// Whether the native device can be created for a toolkit id. Toolkit 3
    /// is never available (see the module notes).
    pub fn toolkit_available(&self, id: i32) -> bool {
        match id {
            // The software toolkit has no failure path.
            0 => true,
            1 | 5 => true,
            3 => self.options.profile.jagdx,
            _ => false,
        }
    }

    /// Queues a graphics report, keeping at most eleven pending and dropping
    /// the oldest.
    pub fn queue_graphics_packet(&mut self, packet: Vec<u8>) {
        while self.graphics_packets.len() > 10 {
            self.graphics_packets.remove(0);
        }
        self.graphics_packets.push(packet);
    }

    /// Queues a toolkit-change report: opcode 53 and the type byte plus 128.
    pub fn queue_toolkit_change(&mut self, toolkit_type: i32) {
        self.queue_graphics_packet(vec![
            crate::proto::client::SIMPLE_TOOLKIT_CHANGE,
            toolkit_type.wrapping_add(128) as u8,
        ]);
    }

    /// The pending reports as one buffer, sent only in client states 18 and 3
    /// with a game connection.
    pub fn flush_graphics_packets(&mut self, state: i32, connected: bool) -> Vec<u8> {
        if (state != 18 && state != 3) || !connected {
            return Vec::new();
        }
        self.graphics_packets.drain(..).flatten().collect()
    }

    /// Saves the preferences immediately (safe-mode crash detection depends
    /// on the file being on disk).
    pub(super) fn save_now(&mut self) {
        self.dirty = true;
        if let Err(error) = self.save() {
            log::warn!("[client910] save preferences: {error}");
        }
    }

    /// Turns bloom on or off against the native device; false when the
    /// device cannot apply the change.
    pub fn set_bloom(&mut self, wanted: bool) -> bool {
        let was = self.bloom_enabled;
        if wanted == was {
            return true;
        }
        let applied = wanted && self.bloom;
        self.bloom_enabled = applied;
        if applied == was {
            return false;
        }
        self.options.set_field("bloom", i32::from(applied)).unwrap();
        self.save_now();
        true
    }

    /// Switches toolkit: the old device is deleted and recreated, then the
    /// model caches reset and the screen redraws (the queued `RecreateToolkit`
    /// consumer).
    pub fn set_toolkit(&mut self, id: i32, profiling: bool) {
        self.create_toolkit(id, profiling);
        self.pending_effects.push(PreferenceEffect::RecreateToolkit);
    }

    /// The developer console's toolkit commands: switch to `id` and, when
    /// the toolkit took, save it as the startup toolkit too. False when the
    /// toolkit could not be created (the previous one stays).
    pub fn console_set_toolkit(&mut self, id: i32) -> bool {
        self.set_toolkit(id, false);
        if self.options.get("displayMode") != Some(id) {
            return false;
        }
        self.options.set_field("toolkit", id);
        self.save_now();
        self.change_notified = false;
        true
    }

    /// The GPU device was made again (after a fault) under the same toolkit:
    /// the scene is built again, the server is told the graphics state and
    /// texture formats again, and the environment fade starts over. (The
    /// device itself is already new, so no toolkit change is queued.)
    pub fn device_recreated(&mut self) {
        self.change_notified = false;
        self.texture_formats_sent = false;
        self.window.changed = true;
        self.pending_effects
            .push(PreferenceEffect::ResetModelCaches);
        self.pending_effects
            .push(PreferenceEffect::ResetEnvironmentFade);
    }

    /// The preference and device-state side of creating a toolkit.
    pub fn create_toolkit(&mut self, id: i32, profiling: bool) {
        if !self.toolkit_available(id) {
            // Report the failure, then fall back to the previously active
            // toolkit after recording displayMode 0. When the previous one is
            // the failed toolkit itself, the second attempt fails the same way
            // and lands on the software toolkit.
            match id {
                1 => self.queue_toolkit_change(toolkit_type::GL_FAILED),
                3 => self.queue_toolkit_change(toolkit_type::DX_FAILED),
                _ => {}
            }
            let previous = self.options.get("displayMode").unwrap();
            self.options.set_field("displayMode", 0);
            self.create_toolkit(previous, profiling);
            return;
        }
        // Success tail.
        if profiling {
            self.options.set_display_flag("displayMode", !profiling);
        }
        self.options.set_field("displayMode", id);
        if !profiling {
            self.options.set_display_flag("displayMode", !profiling);
        }
        // The device sample count is `antiAliasing2 * 2`, fixed at creation; later preference writes wait for the next one.
        self.active_aa = self.options.get("antiAliasing2").unwrap();
        // A fresh toolkit starts with bloom disabled; the software toolkit
        // never supports it.
        self.bloom_enabled = false;
        if self.bloom && id != 0 {
            self.set_bloom(self.options.get("bloom") == Some(1));
        }
        self.pending_effects
            .push(PreferenceEffect::ResetEnvironmentFade);
        // The server is told about the change and the texture formats again.
        self.change_notified = false;
        self.texture_formats_sent = false;
        // The canvas is marked changed and the fullscreen modes are forgotten.
        self.window.changed = true;
        self.window.forget_modes();
    }

    /// The safe-mode recovery at startup, right after the options load: a
    /// crash marker left in `safeMode` is recorded and reported.
    pub fn maininit_safe_mode(&mut self) {
        match self.options.get("safeMode") {
            Some(3) => {
                self.blackflag_mode3 = true;
                self.options.set_field("safeMode", 0);
                self.queue_toolkit_change(toolkit_type::DX_CRASHED);
            }
            Some(4) => {
                self.blackflag_mode4 = true;
                self.options.set_field("safeMode", 0);
                self.queue_toolkit_change(toolkit_type::GL_CRASHED);
            }
            _ => {}
        }
    }

    /// Selects and creates the startup toolkit. The window mode half is
    /// applied by the window owner. `cpu_info_ram` is the machine's RAM in
    /// megabytes (0 without the native probe).
    #[allow(
        clippy::if_same_then_else,
        reason = "the two software-toolkit conditions are kept as separate branches (same body twice)"
    )]
    pub fn loading_toolkit(&mut self, cpu_info_ram: i32) {
        self.is_safe_mode = self.options.get("safeMode") == Some(1);
        self.options.set_field("safeMode", 1);
        let slot = crate::client_options::ClientOptions::field_index("toolkit").unwrap();
        let defaulted = self.options.defaulted[slot];
        if self.is_safe_mode {
            self.options.set_field("toolkit", 0);
        } else if defaulted && cpu_info_ram < 512 && cpu_info_ram != 0 {
            self.options.set_field("toolkit", 0);
        }
        self.save_now();
        if self.is_safe_mode {
            self.create_toolkit(0, false);
            if !self.chose_safe_mode {
                self.queue_toolkit_change(toolkit_type::SAFE_MODE);
            }
        } else {
            let saved = self.options.get("toolkit").unwrap();
            self.create_toolkit(saved, false);
            if self.options.get("toolkit") == Some(0) {
                self.queue_toolkit_change(toolkit_type::SOFTWARE_SAVED);
            }
        }
    }

    /// Confirms a safe-mode start once the title screen shows a top-level
    /// interface, evaluated at the end of each main loop.
    pub fn confirm_safe_mode(&mut self, state: i32, top_level_open: bool) {
        if self.options.get("safeMode") == Some(1) && state == 4 && top_level_open {
            self.options.set_field("safeMode", 0);
            self.save_now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn prefs() -> Preferences {
        Preferences {
            bloom: true,
            anti_aliasing: true,
            dirty: false,
            ..Default::default()
        }
    }

    #[test]
    fn detail_toolkit_recreates_supported_devices_and_falls_back() {
        let mut p = prefs();
        p.options.set_field("displayMode", 5).unwrap();
        p.options.set_field("antiAliasing2", 1).unwrap();
        p.dispatch("detail_toolkit", &mut vec![1]).unwrap().unwrap();
        assert_eq!(p.options.get("displayMode"), Some(1));
        assert_eq!(p.active_aa, 1);
        let effects = p.drain_effects();
        assert!(effects.contains(&PreferenceEffect::RecreateToolkit));
        assert!(effects.contains(&PreferenceEffect::ResetEnvironmentFade));
        assert!(p.window.changed);
        // An id of 2 or out of range becomes 3; DirectX fails without jagdx
        // and the previous toolkit is restored.
        p.window.changed = false;
        p.dispatch("detail_toolkit", &mut vec![2]).unwrap().unwrap();
        assert_eq!(p.options.get("displayMode"), Some(1));
        assert_eq!(
            p.graphics_packets,
            vec![vec![crate::proto::client::SIMPLE_TOOLKIT_CHANGE, 132]]
        );
        assert_eq!(p.flush_graphics_packets(13, true), Vec::<u8>::new());
        assert_eq!(p.flush_graphics_packets(18, true), vec![53, 132]);
        assert!(p.graphics_packets.is_empty());
    }

    /// A file saved with the DirectX toolkit (say, by the original client on
    /// Windows) is read as saved. Startup then follows the failure path of a
    /// machine whose DirectX device cannot be created: the failure is
    /// reported, the toolkit that was active (the loading screens' software
    /// toolkit) is created again, and the saved value stays 3 on disk.
    #[test]
    fn saved_directx_toolkit_starts_on_the_software_toolkit_and_stays_saved() {
        let path =
            std::env::temp_dir().join(format!("alto-saved-directx-{}.dat", std::process::id()));
        let mut saved = prefs();
        for name in ["toolkit", "displayMode"] {
            let slot = crate::client_options::ClientOptions::field_index(name).unwrap();
            saved.options.values[slot] = 3;
        }
        saved.options.save(&path).unwrap();

        let mut p = prefs();
        p.install(path.clone());
        assert_eq!(
            p.options.get("toolkit"),
            Some(3),
            "the file is read as saved"
        );
        // The loading stage creates the software toolkit for the loading
        // screens first, then the saved toolkit.
        p.create_toolkit(0, true);
        p.loading_toolkit(0);
        assert_eq!(p.options.get("displayMode"), Some(0));
        assert_eq!(p.options.get("toolkit"), Some(3));
        assert_eq!(
            p.graphics_packets,
            vec![vec![
                crate::proto::client::SIMPLE_TOOLKIT_CHANGE,
                128 + toolkit_type::DX_FAILED as u8
            ]],
            "one failure report, and no software-saved report"
        );
        p.dirty = true;
        p.save().unwrap();
        let reloaded = crate::client_options::ClientOptions::load(&path, p.options.profile);
        assert_eq!(reloaded.get("toolkit"), Some(3));
        let _ = std::fs::remove_file(&path);
    }

    /// The console's `tk` commands: a toolkit that can be created becomes
    /// the active and the startup toolkit and queues the device change; one
    /// that cannot (DirectX is never offered) is reported to the server and
    /// the previous toolkit stays.
    #[test]
    fn console_toolkit_commands_save_what_took_and_refuse_the_rest() {
        let mut p = prefs();
        p.options.set_field("displayMode", 5).unwrap();
        p.options.set_field("toolkit", 5).unwrap();
        assert!(p.console_set_toolkit(1));
        assert_eq!(p.options.get("displayMode"), Some(1));
        assert_eq!(p.options.get("toolkit"), Some(1));
        assert!(p
            .drain_effects()
            .contains(&PreferenceEffect::RecreateToolkit));
        assert!(!p.console_set_toolkit(3));
        assert_eq!(p.options.get("displayMode"), Some(1));
        assert_eq!(p.options.get("toolkit"), Some(1));
        assert_eq!(
            p.graphics_packets,
            vec![vec![
                crate::proto::client::SIMPLE_TOOLKIT_CHANGE,
                128 + toolkit_type::DX_FAILED as u8
            ]]
        );
    }

    /// The client runs with the default profile on every operating system:
    /// toolkit 3 is refused, GL is the default toolkit.
    #[test]
    fn the_machine_profile_never_offers_directx() {
        let profile = crate::client_options::Profile::default();
        assert!(!profile.windows && !profile.jagdx);
        let p = prefs();
        assert!(!p.toolkit_available(3));
        assert_eq!(p.options.can_set("toolkit", 3), Some(3));
        assert_eq!(p.options.can_set("displayMode", 3), Some(3));
        assert_eq!(
            crate::client_options::ClientOptions::new(profile).get("toolkit"),
            Some(1)
        );
    }

    #[test]
    fn detail_antialiasing_takes_effect_at_toolkit_creation() {
        let mut p = prefs();
        p.options.set_field("displayMode", 5).unwrap();
        p.dispatch("detail_antialiasing", &mut vec![2])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("antiAliasing2"), Some(2));
        assert_eq!(p.active_aa, 2);
        assert!(p
            .drain_effects()
            .contains(&PreferenceEffect::RecreateToolkit));
        // Presets write antiAliasing2 without recreating the toolkit, so the
        // device keeps its samples until the next creation.
        p.dispatch("autosetup_setlow", &mut vec![])
            .unwrap()
            .unwrap();
        assert_eq!(p.options.get("antiAliasing2"), Some(0));
        assert_eq!(p.active_aa, 2);
    }

    #[test]
    fn toolkit_recreation_reapplies_saved_bloom_and_presets_disable_it() {
        let mut p = prefs();
        p.options.set_field("displayMode", 5).unwrap();
        p.dispatch("detail_bloom", &mut vec![1]).unwrap().unwrap();
        assert!(p.bloom_enabled);
        p.dispatch("detail_toolkit", &mut vec![5]).unwrap().unwrap();
        assert!(
            p.bloom_enabled,
            "creating a toolkit re-applies the saved bloom"
        );
        p.drain_effects();
        // The high preset disables the device bloom.
        p.dispatch("autosetup_sethigh", &mut vec![])
            .unwrap()
            .unwrap();
        assert!(!p.bloom_enabled);
        assert_eq!(p.options.get("bloom"), Some(0));
        let effects = p.drain_effects();
        assert!(effects.contains(&PreferenceEffect::ResetEnvironmentFade));
        assert!(p.window.changed, "the canvas is marked changed");
    }

    #[test]
    fn startup_safe_mode_lifecycle_matches_loading_and_mainloop() {
        let mut p = prefs();
        p.options.set_field("safeMode", 4).unwrap();
        p.maininit_safe_mode();
        assert!(p.blackflag_mode4);
        assert_eq!(p.options.get("safeMode"), Some(0));
        p.loading_toolkit(0);
        assert!(!p.is_safe_mode);
        assert_eq!(p.options.get("safeMode"), Some(1));
        assert_eq!(p.options.get("displayMode"), p.options.get("toolkit"));
        // Only the title screen with an open top-level interface confirms.
        p.confirm_safe_mode(18, true);
        assert_eq!(p.options.get("safeMode"), Some(1));
        p.confirm_safe_mode(4, true);
        assert_eq!(p.options.get("safeMode"), Some(0));
        // A crash before confirmation starts the next run in safe mode.
        let mut next = prefs();
        next.options.set_field("safeMode", 1).unwrap();
        next.loading_toolkit(0);
        assert!(next.is_safe_mode);
        assert_eq!(next.options.get("toolkit"), Some(0));
        assert_eq!(next.options.get("displayMode"), Some(0));
        assert!(next
            .graphics_packets
            .contains(&vec![crate::proto::client::SIMPLE_TOOLKIT_CHANGE, 128 + 9]));
    }
}
