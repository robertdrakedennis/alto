//! Quality presets and autosetup rules over the [`ClientOptions`] scalar rules
//! (kept apart from `client_options.rs`).
use super::ClientOptions;
impl ClientOptions {
    /// Sets the CPU usage option from the machine profile: 4 on a multi-core
    /// machine, else 2. Goes through `set_field`, so the full clamp runs. This
    /// is also the default for the CPU usage slot.
    pub fn set_cpu_usage_preference(&mut self) {
        let value = if self.profile.cpu_count > 1 { 4 } else { 2 };
        self.set_field("cpuUsage", value);
    }
    /// The high quality preset, applied in this order through `set_field` (each
    /// step clamps). The build area is the standard size. The toolkit-specific
    /// bloom disabling is device state with no owner here, so only the `bloom 0`
    /// option is recorded; the model-cache and fade resets are queued by the
    /// caller that owns the options, not here.
    pub fn apply_high_preset(&mut self) {
        self.set_field("removeRoofs", 2);
        self.set_field("removeRoofs2", 2);
        self.set_field("groundDecoration", 1);
        self.set_field("groundBlending", 1);
        self.set_field("idleAnimations", 1);
        self.set_field("flickeringEffects", 1);
        self.set_field("characterShadows", 1);
        self.set_field("textures", 1);
        self.set_field("sceneryShadows", 2);
        self.set_field("lightingDetail", 1);
        self.set_field("waterDetail", 2);
        self.set_field("fog", 1);
        self.set_field("antiAliasing", 0);
        self.set_field("antiAliasing2", 0);
        self.set_field("particles", 2);
        self.set_field("buildArea", 0);
        self.set_field("bloom", 0);
        self.set_field("skyboxes", 1);
        self.set_field("animDetail", 1);
        self.set_cpu_usage_preference();
        self.set_field("screenSize", 0);
        self.set_field("preset", 4);
    }
    /// The medium preset, applied in order. Differs from high in
    /// `sceneryShadows 1`, `waterDetail 0`, `particles 1`, `screenSize 1` and
    /// `preset 3`. The bloom and model-cache notes of `apply_high_preset` apply.
    pub fn apply_medium_preset(&mut self) {
        self.set_field("removeRoofs", 2);
        self.set_field("removeRoofs2", 2);
        self.set_field("groundDecoration", 1);
        self.set_field("groundBlending", 1);
        self.set_field("idleAnimations", 1);
        self.set_field("flickeringEffects", 1);
        self.set_field("characterShadows", 1);
        self.set_field("textures", 1);
        self.set_field("sceneryShadows", 1);
        self.set_field("lightingDetail", 1);
        self.set_field("waterDetail", 0);
        self.set_field("fog", 1);
        self.set_field("antiAliasing", 0);
        self.set_field("antiAliasing2", 0);
        self.set_field("particles", 1);
        self.set_field("buildArea", 0);
        self.set_field("bloom", 0);
        self.set_field("skyboxes", 1);
        self.set_field("animDetail", 1);
        self.set_cpu_usage_preference();
        self.set_field("screenSize", 1);
        self.set_field("preset", 3);
    }
    /// The low quality preset, applied in order.
    pub fn apply_low_preset(&mut self) {
        self.set_field("removeRoofs", 1);
        self.set_field("removeRoofs2", 1);
        self.set_field("groundDecoration", 1);
        self.set_field("groundBlending", 1);
        self.set_field("idleAnimations", 0);
        self.set_field("flickeringEffects", 0);
        self.set_field("characterShadows", 0);
        self.set_field("sceneryShadows", 0);
        self.set_field("textures", 0);
        self.set_field("lightingDetail", 0);
        self.set_field("waterDetail", 0);
        self.set_field("fog", 0);
        self.set_field("antiAliasing", 0);
        self.set_field("antiAliasing2", 0);
        self.set_field("particles", 0);
        self.set_field("buildArea", 0);
        self.set_field("bloom", 0);
        self.set_field("skyboxes", 0);
        self.set_field("animDetail", 0);
        self.set_cpu_usage_preference();
        self.set_field("screenSize", 2);
        self.set_field("preset", 2);
    }
    /// The minimum quality preset, applied in order.
    pub fn apply_min_preset(&mut self) {
        self.set_field("removeRoofs", 1);
        self.set_field("removeRoofs2", 1);
        self.set_field("groundDecoration", 0);
        self.set_field("fog", 0);
        self.set_field("groundBlending", 0);
        self.set_field("idleAnimations", 0);
        self.set_field("flickeringEffects", 0);
        self.set_field("characterShadows", 0);
        self.set_field("sceneryShadows", 0);
        self.set_field("textures", 0);
        self.set_field("lightingDetail", 0);
        self.set_field("waterDetail", 0);
        self.set_field("antiAliasing", 0);
        self.set_field("antiAliasing2", 0);
        self.set_field("particles", 0);
        self.set_field("buildArea", 0);
        self.set_field("bloom", 0);
        self.set_field("skyboxes", 0);
        self.set_field("animDetail", 0);
        self.set_cpu_usage_preference();
        self.set_field("screenSize", 2);
        self.set_field("preset", 1);
    }
    /// Picks a preset from a CPU benchmark. Less than 96 MB of memory forces
    /// preset 1; otherwise the benchmark's millisecond result maps `<=100 -> 4`,
    /// `<=500 -> 3`, `<=1003 -> 2`, else `1`. Running the benchmark and applying
    /// the display mode and toolkit belong to the device; this returns only the
    /// preset id.
    pub fn autosetup_preset_for_cpu_profile(cpu_ms: i32, maxmemory_mb: i32) -> i32 {
        if maxmemory_mb < 96 {
            return 1;
        }
        if cpu_ms <= 100 {
            4
        } else if cpu_ms <= 500 {
            3
        } else if cpu_ms <= 1003 {
            2
        } else {
            1
        }
    }
    /// Picks a preset from a performance score: above 100000 is 4, above 50000
    /// is 3, above 10000 is 2, else 1. Applying the display mode and toolkit
    /// belongs to the device.
    pub fn autosetup_preset_for_metric(metric: i32) -> i32 {
        if metric > 100000 {
            4
        } else if metric > 50000 {
            3
        } else if metric > 10000 {
            2
        } else {
            1
        }
    }
    /// Applies an autosetup result as pure state. `0` (custom) changes no
    /// graphics options: only the explicit "set custom" script command writes
    /// `preset 0`.
    pub fn apply_preset(&mut self, preset: i32) {
        match preset {
            1 => self.apply_min_preset(),
            2 => self.apply_low_preset(),
            3 => self.apply_medium_preset(),
            4 => self.apply_high_preset(),
            _ => {}
        }
    }
}
