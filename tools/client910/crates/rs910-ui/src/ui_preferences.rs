//! The single client preference owner shared by script queries and persistence.
//! The file lifecycle delegates to `ClientOptions::load/save`: a load that
//! fails for any reason returns the defaults, and a save writes the encoded
//! options block. The atomic temp-file rename lives in the codec; callers pass
//! temp-dir paths only.
use crate::client_options::{ClientOptions, Profile};
use native910::vm::{Value, VmError, VmResult};
use std::path::PathBuf;

pub const VOLUMES: [&str; 5] = [
    "soundVolume",
    "backgroundSoundVolume",
    "speechVolume",
    "unknownVolume1",
    "unknownVolume2",
];
pub struct Preferences {
    pub options: ClientOptions,
    /// What the machine probe reported (all unknown until the shell calls
    /// [`Preferences::apply_hardware`]).
    pub hardware: rs910_core::hardware::Hardware,
    pub window: crate::ui_window::WindowState,
    pub path: Option<PathBuf>,
    pub dirty: bool,
    /// Current implementation capabilities. Device negotiation and the missing
    /// graphics effects remain cutover work; these flags are not parity proof.
    pub anti_aliasing: bool,
    pub bloom: bool,
    pub bloom_enabled: bool,
    /// The display mode the auto-setup reports, separate from the saved
    /// `displayMode` option.
    pub autosetup_display_mode: i32,
    /// Crash markers of toolkits 3 and 4, set by `autosetup_blackflaglast` and
    /// safe-mode recovery; the auto-setup probe in `ui_preferences_autosetup.rs`
    /// skips a marked toolkit.
    pub blackflag_mode3: bool,
    pub blackflag_mode4: bool,
    /// Marks toolkit 5 as not worth probing. The 910 client never sets it;
    /// the auto-setup probe honours it.
    pub skip_toolkit5_probe: bool,
    /// The client started in safe mode, and the player's explicit safe-mode
    /// choice (never made by the 910 client).
    pub is_safe_mode: bool,
    pub chose_safe_mode: bool,
    /// The `antiAliasing2` preference captured by the last toolkit creation:
    /// the active device sample level.
    pub active_aa: i32,
    /// Encoded graphics reports waiting to be sent to the server.
    pub graphics_packets: Vec<Vec<u8>>,
    /// The `performancemetricsmodel` value of the cache's graphics defaults.
    pub performance_metrics_model: Option<i32>,
    /// The PCI vendor id and driver version of the DirectX toolkit's device,
    /// read after a successful toolkit 3 probe. This client never has a
    /// DirectX toolkit, so it stays unset.
    pub toolkit3_driver: Option<(u32, u64)>,
    /// Inputs the performance metric reads from the client: the cache root
    /// for the benchmark model and the map width for the clip distances.
    pub metric_context: Option<MetricContext>,
    pub metric_model: Option<std::sync::Arc<metric::MetricModel>>,
    /// Effects requested by preference commands. Consumers drain this
    /// queue at the root that owns the renderer/window; preference mutation
    /// never pretends to apply those effects itself.
    pub pending_effects: Vec<PreferenceEffect>,
    /// Whether the server has been told about the current preferences
    /// (default true): cleared by a toolkit change and by the saving
    /// `detail_*`/`autosetup_*` commands; the telemetry report sends the
    /// block and sets it.
    pub change_notified: bool,
    /// Whether the supported compressed texture formats have been sent
    /// (default true), cleared by a toolkit change.
    pub texture_formats_sent: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        // Provisional GLX profile for the current renderer. Runtime hardware
        // negotiation remains open. `ClientOptions::new(Profile)` builds the
        // defaults of an explicit capability profile for the oracle.
        let mut options = ClientOptions::new(Profile {
            initial_display_mode: 5,
            ..Profile::default()
        });
        options.set_field("displayMode", 5);
        Self {
            options,
            hardware: Default::default(),
            window: Default::default(),
            path: None,
            dirty: false,
            anti_aliasing: false,
            bloom: false,
            bloom_enabled: false,
            autosetup_display_mode: 0,
            blackflag_mode3: false,
            blackflag_mode4: false,
            skip_toolkit5_probe: false,
            is_safe_mode: false,
            chose_safe_mode: false,
            active_aa: 0,
            graphics_packets: Vec::new(),
            performance_metrics_model: None,
            toolkit3_driver: None,
            metric_context: None,
            metric_model: None,
            pending_effects: Vec::new(),
            change_notified: true,
            texture_formats_sent: true,
        }
    }
}
#[path = "ui_preferences_mutations.rs"]
mod mutations;
pub use mutations::{PreferenceEffect, QualityLevel};
#[path = "ui_preferences_autosetup.rs"]
mod autosetup;
#[path = "ui_preferences_toolkit.rs"]
mod toolkit;
pub use autosetup::{cpu_profile, AutoSetupResult, ProbeSlot, Profiler};
pub use rs910_toolkit::performance_metric as metric;
/// What the performance metric reads from the client: the cache root for the
/// benchmark model and the map width for the clip distances. The benchmark
/// itself runs on the renderer the shell lends the host
/// ([`metric::RendererProbe`]), never on anything this context holds.
#[derive(Clone, Debug)]
pub struct MetricContext {
    pub pack_root: PathBuf,
    pub map_size_x: i32,
}
fn pop(ints: &mut Vec<i32>) -> VmResult<i32> {
    ints.pop().ok_or(VmError::StackUnderflow { stack: "int" })
}
impl Preferences {
    /// Adopts the machine probe: the capability profile the option rules read
    /// (memory budget, processor count) and the figures scripts and the login
    /// block report. Runs before [`Self::install`], which decodes the saved
    /// options under that profile.
    pub fn apply_hardware(&mut self, hardware: rs910_core::hardware::Hardware) {
        let mut profile = self.options.profile;
        profile.max_memory_mb = hardware.profile_memory_mb();
        profile.cpu_count = hardware.profile_cpu_count();
        self.options = ClientOptions::new(profile);
        self.options.set_field("displayMode", 5);
        self.hardware = hardware;
    }

    pub fn install(&mut self, path: PathBuf) {
        self.options = ClientOptions::load(&path, self.options.profile);
        // The active toolkit (`displayMode`) is chosen by the Loading stage
        // (`loading_toolkit`) once the native device exists.
        self.active_aa = self.options.get("antiAliasing2").unwrap();
        if crate::toolkit_debug_flags::flags().settings_trace {
            log::info!(
                "[settings] loaded {}: screenSize={:?}, windowMode={:?}",
                path.display(),
                self.options.get("screenSize"),
                self.options.get("windowMode")
            );
        }
        self.path = Some(path);
        self.dirty = false;
        // Safe-mode recovery runs directly after the options load.
        self.maininit_safe_mode();
    }
    pub fn save(&mut self) -> std::io::Result<()> {
        if self.dirty {
            if let Some(path) = &self.path {
                self.options.save(path)?;
                self.dirty = false;
            }
        }
        Ok(())
    }
    pub fn dispatch(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
    ) -> Option<VmResult<Option<Value>>> {
        self.dispatch_profiling(command, ints, None)
    }
    /// [`Self::dispatch`] with the renderer the profiling commands measure
    /// on (`autosetup_dosetup`, `detailget_performance_metric`); without one
    /// their device benchmarks fail as a missing toolkit does.
    pub fn dispatch_profiling(
        &mut self,
        command: &str,
        ints: &mut Vec<i32>,
        profiler: Option<&mut Profiler<'_>>,
    ) -> Option<VmResult<Option<Value>>> {
        if let Some(result) =
            self.window
                .dispatch(command, ints, &mut self.options, &mut self.dirty)
        {
            return Some(result);
        }
        if let Some(result) = self.mutate_graphics(command, ints, profiler) {
            return Some(result);
        }
        if command == "detailget_particles" {
            return Some(Ok(Some(Value::Int(self.options.particle_level))));
        }
        if let Some(result) = self.query(command, ints) {
            return Some(result);
        }
        let capability = match command {
            "detailcanmod_antialiasing" | "detailcanset_antialiasing" => {
                Some(("antiAliasing", self.anti_aliasing))
            }
            "detailcanmod_bloom" | "detailcanset_bloom" => Some(("bloom", self.bloom)),
            _ => None,
        };
        if let Some((field, supported)) = capability {
            return Some((|| {
                if command.starts_with("detailcanmod") {
                    Ok(Some(Value::Int(i32::from(
                        supported && self.options.can_mod(field).unwrap(),
                    ))))
                } else {
                    let value = pop(ints)?;
                    Ok(Some(Value::Int(if supported {
                        self.options.can_set(field, value).unwrap()
                    } else {
                        3
                    })))
                }
            })());
        }
        let volume = match command {
            "detail_soundvol" => Some(0),
            "detail_bgsoundvol" => Some(1),
            "detail_speechvol" => Some(2),
            "detail_musicvol" => Some(3),
            "detail_loginvol" => Some(4),
            _ => None,
        };
        if let Some(slot) = volume {
            return Some((|| {
                let value = pop(ints)?;
                if crate::toolkit_debug_flags::flags().settings_trace {
                    log::info!("[settings] {command} {value}");
                }
                let field = VOLUMES[slot];
                if slot < 3 || self.options.get(field) != Some(value) {
                    self.options.set_field(field, value).unwrap();
                    self.dirty = true;
                    // Changing a volume marks the preferences as unreported.
                    self.change_notified = false;
                }
                Ok(None)
            })());
        }
        None
    }
}
#[path = "ui_preferences_queries.rs"]
mod queries;

#[cfg(test)]
mod tests {
    use super::*;
    use rs910_core::hardware::Hardware;

    /// The machine probe sets the capability profile the option rules read:
    /// a small machine has no particle detail and cannot change it, a large
    /// one defaults to the highest level.
    #[test]
    fn the_probed_machine_sets_the_option_profile() {
        let machine = |ram_mb, cpus| Hardware {
            ram_mb,
            memory_budget_mb: ram_mb / 4,
            logical_cpus: cpus,
            ..Hardware::default()
        };
        let mut small = Preferences::default();
        small.apply_hardware(machine(400, 1));
        assert_eq!(small.options.profile.max_memory_mb, 100);
        assert_eq!(small.options.profile.cpu_count, 1);
        assert_eq!(small.options.get("particles"), Some(0));
        assert!(!small.options.can_mod("particles").unwrap());
        let mut large = Preferences::default();
        large.apply_hardware(machine(32_768, 10));
        assert_eq!(large.options.profile.cpu_count, 10);
        assert_eq!(large.options.get("particles"), Some(2));
        assert!(large.options.can_mod("particles").unwrap());
        assert_eq!(large.hardware.ram_mb, 32_768);
    }
}
