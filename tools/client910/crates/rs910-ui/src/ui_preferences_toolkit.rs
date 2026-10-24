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
