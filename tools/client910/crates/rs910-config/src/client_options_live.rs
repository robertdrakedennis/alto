//! The saved options as the client's live consumers read them.
//!
//! Every renderer, window, input and scene owner that reacts to a saved
//! option reads it through [`LiveSettings`], never through the option's
//! name: one place decides which slot feeds which consumer, a renamed or
//! mistyped slot fails to compile instead of panicking mid-frame, and the
//! settings tests can compare two snapshots to prove a control reached its
//! consumers. Options that only the preference owner itself acts on (the
//! toolkit and anti-aliasing device state, the safe-mode markers, the
//! volumes the audio owner reads) are not part of this view.
use super::{ClientOptions, FIELDS};

/// The slot of a named option in the persisted table.
const fn slot(name: &str) -> usize {
    let wanted = name.as_bytes();
    let mut i = 0;
    while i < FIELDS.len() {
        let field = FIELDS[i].as_bytes();
        if field.len() == wanted.len() {
            let mut k = 0;
            while k < field.len() && field[k] == wanted[k] {
                k += 1;
            }
            if k == field.len() {
                return i;
            }
        }
        i += 1;
    }
    panic!("unknown option name")
}

const BRIGHTNESS: usize = slot("brightness");
const FOG: usize = slot("fog");
const SKYBOXES: usize = slot("skyboxes");
const ROOFS: usize = slot("removeRoofs2");
const IDLE_ANIMATIONS: usize = slot("idleAnimations");
const SCENERY_SHADOWS: usize = slot("sceneryShadows");
const SHADOW_QUALITY: usize = slot("shadowQuality");
const CHARACTER_SHADOWS: usize = slot("characterShadows");
const FLICKERING: usize = slot("flickeringEffects");
const CUSTOM_CURSORS: usize = slot("customCursors");
const CPU_USAGE: usize = slot("cpuUsage");
const SCREEN_SIZE: usize = slot("screenSize");
const LOADING_SCREEN: usize = slot("loadingScreen");
const BUILD_AREA: usize = slot("buildArea");
const CONSOLE_KEY: usize = slot("consoleKeyPress");
const WINDOW_MODE: usize = slot("windowMode");

/// The live view of the saved options. `Default` is what the consumers use
/// before a session has options: the client's stock quality.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LiveSettings {
    /// The brightness step that scales the sun and ambient light.
    pub brightness: i32,
    /// Distance fog.
    pub fog: bool,
    /// The skybox detail level (0 off).
    pub sky_detail: i32,
    /// The roof removal mode the scene draw applies.
    pub roof_mode: i32,
    /// How many idle animations players play (0 none, 1 many).
    pub idle_animation_detail: i32,
    /// The scenery shadow mode (0 off, 1 static, 2 dynamic).
    pub scenery_shadows: i32,
    /// The sun shadow quality step.
    pub shadow_quality: i32,
    /// Character (spot) shadows.
    pub character_shadows: bool,
    /// Animated flickering lights.
    pub flickering: bool,
    /// The particle detail level, as the particle system resizes itself to
    /// it (0 to 2; an out-of-range value reads as 0).
    pub particle_level: i32,
    /// Custom cursor images.
    pub custom_cursors: bool,
    /// The CPU usage step of the post-frame sleep schedule.
    pub cpu_usage: i32,
    /// The maximum screen size step that bounds the canvas.
    pub screen_size: i32,
    /// The loading screen variant.
    pub loading_screen: i32,
    /// The build area size the title world uses.
    pub build_area: i32,
    /// The key code that opens the console.
    pub console_key: i32,
    /// The saved window mode (1 fixed canvas, 2 resizable).
    pub window_mode: i32,
}

impl Default for LiveSettings {
    fn default() -> Self {
        Self {
            brightness: 3,
            fog: true,
            sky_detail: 1,
            roof_mode: 2,
            idle_animation_detail: 1,
            scenery_shadows: 2,
            shadow_quality: 1,
            character_shadows: true,
            flickering: true,
            particle_level: 2,
            custom_cursors: true,
            cpu_usage: 4,
            screen_size: 0,
            loading_screen: 0,
            build_area: 0,
            console_key: 0,
            window_mode: 2,
        }
    }
}

impl ClientOptions {
    /// The options as the live consumers read them.
    #[must_use]
    pub fn live(&self) -> LiveSettings {
        let value = |slot: usize| self.values[slot];
        LiveSettings {
            brightness: value(BRIGHTNESS),
            fog: value(FOG) == 1,
            sky_detail: value(SKYBOXES),
            roof_mode: value(ROOFS),
            idle_animation_detail: value(IDLE_ANIMATIONS),
            scenery_shadows: value(SCENERY_SHADOWS),
            shadow_quality: value(SHADOW_QUALITY),
            character_shadows: value(CHARACTER_SHADOWS) == 1,
            flickering: value(FLICKERING) == 1,
            particle_level: self.particle_level,
            custom_cursors: value(CUSTOM_CURSORS) == 1,
            cpu_usage: value(CPU_USAGE),
            screen_size: value(SCREEN_SIZE),
            loading_screen: value(LOADING_SCREEN),
            build_area: value(BUILD_AREA),
            console_key: value(CONSOLE_KEY),
            window_mode: value(WINDOW_MODE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client_options::Profile;

    #[test]
    fn the_view_follows_the_saved_slots() {
        let mut options = ClientOptions::new(Profile::default());
        let before = options.live();
        options.set_field("brightness", 1);
        options.set_field("fog", 0);
        options.set_field("flickeringEffects", 0);
        let after = options.live();
        assert_eq!(
            (after.brightness, after.fog, after.flickering),
            (1, false, false)
        );
        assert_ne!(before, after);
        // Untouched slots keep reading their own values.
        assert_eq!(after.scenery_shadows, before.scenery_shadows);
    }
}
