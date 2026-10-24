//! The graphics auto-setup probe: each available toolkit is benchmarked, the
//! best one and a quality preset are chosen, and the outcome is reported to
//! the server as an `AUTO_SETUP_RESULT` packet (opcode 112, 18 payload
//! bytes). Also the client's performance metric (a model drawn repeatedly on
//! the device within a time budget) and the CPU probe used when no toolkit
//! could be measured.
use super::{PreferenceEffect, Preferences};
use rs910_toolkit::performance_metric::{Backdrop, Benchmark, RendererProbe};

/// A measurement slot of the auto-setup report. The report carries one
/// number per probed toolkit and the CPU score; the two unused slots are
/// accepted and ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeSlot {
    /// Milliseconds the CPU probe took (clamped to 15 bits).
    CpuScore,
    /// Draws per second measured on toolkit 1 (fixed-function GL).
    Toolkit1,
    /// Draws per second measured on toolkit 3 (DirectX).
    Toolkit3,
    /// Draws per second measured on toolkit 5 (shader GL).
    Toolkit5,
    /// Slots the report does not carry.
    Unused,
}
impl ProbeSlot {
    /// The slot a numbered measurement is filed under.
    pub fn from_index(index: i32) -> Self {
        match index {
            0 => ProbeSlot::CpuScore,
            1 => ProbeSlot::Toolkit1,
            3 => ProbeSlot::Toolkit3,
            5 => ProbeSlot::Toolkit5,
            _ => ProbeSlot::Unused,
        }
    }
}

/// What the auto-setup probe found: capability flags, the chosen toolkit,
/// the measurements and the preset level it settled on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AutoSetupResult {
    /// The CPU probe time in milliseconds, or -1 when it did not run.
    pub cpu_score: i32,
    /// The metric reserved for a fourth probe; never measured, always -1.
    pub reserved_metric: i32,
    /// The toolkit 1 metric, or -1 when it was not measured.
    pub toolkit1_metric: i32,
    /// The toolkit 3 metric, or -1 when it was not measured.
    pub toolkit3_metric: i32,
    /// The toolkit 5 metric, or -1 when it was not measured.
    pub toolkit5_metric: i32,
    /// Capability and skipped-probe bits (see the `FLAG_*` constants).
    pub flags: i32,
    /// The toolkit the measured setup picked (0 when none was measured).
    pub chosen_toolkit: i32,
    /// The preset level applied (0 = custom).
    pub result: i32,
}
impl Default for AutoSetupResult {
    fn default() -> Self {
        Self {
            cpu_score: -1,
            reserved_metric: -1,
            toolkit1_metric: -1,
            toolkit3_metric: -1,
            toolkit5_metric: -1,
            flags: 0,
            chosen_toolkit: 0,
            result: 0,
        }
    }
}
impl AutoSetupResult {
    /// Toolkit 1 ran a benchmark.
    pub const FLAG_TOOLKIT1_OK: i32 = 2;
    /// Toolkit 3 ran a benchmark.
    pub const FLAG_TOOLKIT3_OK: i32 = 4;
    /// The toolkit 1 probe was skipped (it crashed the last time).
    pub const FLAG_TOOLKIT1_SKIPPED: i32 = 16;
    /// The toolkit 3 probe was skipped (it crashed the last time).
    pub const FLAG_TOOLKIT3_SKIPPED: i32 = 32;
    /// The heap was too small to run the CPU probe.
    pub const FLAG_LOW_MEMORY: i32 = 64;
    /// The toolkit 3 driver is an NVIDIA driver older than the one the
    /// toolkit needs (see [`old_driver_flag`]).
    pub const FLAG_TOOLKIT3_OLD_NVIDIA_DRIVER: i32 = 256;
    /// The toolkit 3 driver is an AMD driver older than the one the toolkit
    /// needs.
    pub const FLAG_TOOLKIT3_OLD_AMD_DRIVER: i32 = 512;
    /// The toolkit 1 probe failed outright (not a low benchmark).
    pub const FLAG_TOOLKIT1_FAILED: i32 = 2048;
    /// The toolkit 3 probe failed outright.
    pub const FLAG_TOOLKIT3_FAILED: i32 = 4096;
    /// Toolkit 5 ran a benchmark.
    pub const FLAG_TOOLKIT5_OK: i32 = 8192;
    /// The toolkit 5 probe failed outright.
    pub const FLAG_TOOLKIT5_FAILED: i32 = 32768;
    /// The toolkit 5 probe was skipped.
    pub const FLAG_TOOLKIT5_SKIPPED: i32 = 16384;

    /// Sets capability or skipped-probe bits.
    pub fn add_flags(&mut self, flags: i32) {
        self.flags |= flags;
    }
    /// Records the toolkit the setup settled on.
    pub fn set_chosen_toolkit(&mut self, toolkit: i32) {
        self.chosen_toolkit = toolkit;
    }
    /// Files a measurement, clamping it to what the packet can carry (15 bits
    /// for the CPU score, 23 bits for the others).
    pub fn record_metric(&mut self, slot: ProbeSlot, mut value: i32) {
        if slot == ProbeSlot::CpuScore && value > 32767 {
            value = 32767;
        } else if value > 8388607 {
            value = 8388607;
        }
        match slot {
            ProbeSlot::CpuScore => self.cpu_score = value,
            ProbeSlot::Toolkit1 => self.toolkit1_metric = value,
            ProbeSlot::Toolkit3 => self.toolkit3_metric = value,
            ProbeSlot::Toolkit5 => self.toolkit5_metric = value,
            ProbeSlot::Unused => {}
        }
    }
    /// The `AUTO_SETUP_RESULT` packet: opcode 112 with 18 payload bytes.
    pub fn encode(&self) -> Vec<u8> {
        let p3 = |v: i32| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
        let mut out = vec![crate::proto::client::AUTO_SETUP_RESULT];
        out.extend((self.flags as u16).to_be_bytes());
        out.push(self.chosen_toolkit.wrapping_add(128) as u8);
        for v in [self.reserved_metric, self.toolkit5_metric] {
            // Middle, high, low.
            out.extend([(v >> 8) as u8, (v >> 16) as u8, v as u8]);
        }
        out.extend((self.cpu_score as u16).to_be_bytes());
        out.push(128i32.wrapping_sub(self.result) as u8);
        out.extend(p3(self.toolkit3_metric));
        // High, low, middle.
        let v = self.toolkit1_metric;
        out.extend([(v >> 16) as u8, v as u8, (v >> 8) as u8]);
        out
    }
}

/// The first PCI vendor id the toolkit-3 driver check knows (AMD).
const VENDOR_AMD: u32 = 0x1002;
/// The second (NVIDIA).
const VENDOR_NVIDIA: u32 = 0x10de;

/// The report flag for a toolkit 3 driver older than the toolkit needs, by
/// vendor (`driver_version` is the driver's version number; its 48 low bits
/// are compared). A driver this old also rules the toolkit 1 probe out. None
/// for another vendor or a recent enough driver.
#[must_use]
pub fn old_driver_flag(vendor: u32, driver_version: u64) -> Option<i32> {
    let version = driver_version & 0xFFFF_FFFF_FFFF;
    match vendor {
        VENDOR_AMD if version < 60_129_613_779 => {
            Some(AutoSetupResult::FLAG_TOOLKIT3_OLD_AMD_DRIVER)
        }
        VENDOR_NVIDIA if version < 64_425_238_954 => {
            Some(AutoSetupResult::FLAG_TOOLKIT3_OLD_NVIDIA_DRIVER)
        }
        _ => None,
    }
}

/// A probe that could not run at all: its toolkit could not be set up for the
/// measurement (as opposed to a benchmark that ran and scored low).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeFailed;

/// The CPU score (milliseconds) every machine the client runs on reports:
/// the original timed a software triangle loop that any current machine
/// finishes almost at once, so the answer is fixed at a fast result.
const CPU_PROFILE_MS: i32 = 1;

/// The graphics auto-setup's CPU probe score.
pub fn cpu_profile() -> i32 {
    CPU_PROFILE_MS
}

/// The renderer and the box resources a profiling command is lent for one
/// logic cycle. The original draws a "Profiling..." message box straight to
/// the screen before every device benchmark and then blocks on the
/// measurement; here the box is painted from the interface's own fonts and
/// message box style and handed to the renderer to present, and the
/// benchmark runs on the same renderer, inside the command.
pub struct Profiler<'a> {
    pub renderer: &'a mut dyn RendererProbe,
    pub fonts: Option<&'a crate::ui_fonts::Fonts>,
    pub message_box: &'a crate::message_box::MessageBox,
}
impl Profiler<'_> {
    /// Shows the profiling message box over `backdrop`. A box that cannot be
    /// painted yet (no fonts, sprites still downloading) presents as an empty
    /// plan: the backdrop alone.
    fn announce(&mut self, backdrop: Backdrop) {
        let canvas = self.renderer.canvas_size();
        let mut painter = crate::ui_paint::Painter::new(canvas);
        if let Some(fonts) = self.fonts {
            let drawn = crate::message_box::draw(
                &mut painter,
                fonts,
                self.message_box,
                rs910_core::texts::Msg::Profiling.get(),
                canvas.map(|v| v as i32),
                self.renderer.frame_size(),
            );
            if let Err(error) = drawn {
                log::warn!("[client910] profiling message box: {error:#}");
            }
        }
        if let Err(error) = self
            .renderer
            .present_message_box(painter.finish(), backdrop)
        {
            log::warn!("[client910] profiling message box frame: {error:#}");
        }
    }
}

impl Preferences {
    /// The device benchmark of `toolkit` within `ms` milliseconds: draws per
    /// second of the metric model, 1 when the cache defaults say no model is
    /// needed, or -1 when the toolkit or its inputs are unavailable (or no
    /// renderer was lent).
    pub fn performance_metric(
        &mut self,
        toolkit: i32,
        ms: i32,
        profiler: Option<&mut Profiler<'_>>,
    ) -> i32 {
        self.probe_metric(toolkit, ms, profiler).unwrap_or(-1)
    }

    /// [`Preferences::performance_metric`], telling a probe that could not
    /// run (the graphics defaults are not installed) from one that
    /// scored -1.
    fn probe_metric(
        &mut self,
        toolkit: i32,
        ms: i32,
        profiler: Option<&mut Profiler<'_>>,
    ) -> Result<i32, ProbeFailed> {
        let model = match self.performance_metrics_model {
            Some(-1) => return Ok(1),
            Some(model) => model,
            None => {
                log::info!("[client910] getPerformanceMetric: graphics defaults not installed");
                return Err(ProbeFailed);
            }
        };
        let mut backdrop = Backdrop::LastFrame;
        if toolkit != self.options.get("displayMode").unwrap() {
            // Profiling switches the toolkit like a settings change does; the
            // new toolkit starts on a cleared canvas.
            self.set_toolkit(toolkit, true);
            if self.options.get("displayMode") != Some(toolkit) {
                return Ok(-1);
            }
            backdrop = Backdrop::Black;
        }
        let Some(profiler) = profiler else {
            log::info!("[client910] getPerformanceMetric: no renderer to measure on");
            return Ok(-1);
        };
        profiler.announce(backdrop);
        let Some(context) = self.metric_context.clone() else {
            log::info!("[client910] getPerformanceMetric: client cache not installed");
            return Ok(-1);
        };
        let result = (|| -> anyhow::Result<i32> {
            if self.metric_model.is_none() {
                self.metric_model = Some(std::sync::Arc::new(super::metric::load_model(
                    &context.pack_root,
                    model,
                )?));
            }
            let size = self
                .window
                .canvas(self.options.get("screenSize").unwrap())
                .map_or([765, 553], |c| [c.size[0] as u32, c.size[1] as u32]);
            // The scene's far clip distance for this map size; GL-family
            // toolkits report the extra draw distance.
            let far = if context.map_size_x == 0 {
                430
            } else {
                (f64::from(context.map_size_x) * 34.46) as i32
            } << 2;
            profiler.renderer.benchmark(&Benchmark {
                model: self.metric_model.as_ref().unwrap(),
                canvas: size,
                near: 200.0,
                far: (far + 512) as f32,
                budget_ms: i64::from(ms),
            })
        })();
        let rate = result.unwrap_or_else(|error| {
            // A failed benchmark reports -1.
            log::warn!("[client910] getPerformanceMetric: {error:#}");
            -1
        });
        log::info!("[client910] profiled toolkit {toolkit}: {rate} draws per second in {ms} ms");
        Ok(rate)
    }

    /// Runs the auto-setup, queues its report for the server and returns the
    /// preset level it applied.
    pub fn get_autosetup_result(&mut self, profiler: Option<&mut Profiler<'_>>) -> i32 {
        let result = self.autosetup(profiler);
        if crate::toolkit_debug_flags::flags().settings_trace {
            log::info!("[settings] autosetup {result:?}");
        }
        self.queue_graphics_packet(result.encode());
        result.result
    }

    /// The auto-setup: probes each toolkit that is available and not marked
    /// as crashed, then applies the preset the best measurement selects.
    pub fn autosetup(&mut self, mut profiler: Option<&mut Profiler<'_>>) -> AutoSetupResult {
        let mut r = AutoSetupResult::default();
        // The DirectX probe runs only on a machine with DirectX, which this
        // client is not (see the toolkit notes).
        let mut probe_toolkit3 = self.options.profile.windows;
        let mut probe_toolkit1 = true;
        let mut probe_toolkit5 = true;
        if self.blackflag_mode4 {
            r.add_flags(AutoSetupResult::FLAG_TOOLKIT1_SKIPPED);
            probe_toolkit1 = false;
        }
        if self.blackflag_mode3 {
            r.add_flags(AutoSetupResult::FLAG_TOOLKIT3_SKIPPED);
            probe_toolkit3 = false;
        }
        if self.skip_toolkit5_probe {
            r.add_flags(AutoSetupResult::FLAG_TOOLKIT5_SKIPPED);
            probe_toolkit5 = false;
        }
        if !probe_toolkit1 && !probe_toolkit3 {
            self.apply_unmeasured_setup(&mut r);
            return r;
        }
        let (mut metric1, mut metric3, mut metric5) = (-1, -1, -1);
        if probe_toolkit3 {
            self.options.set_field("safeMode", 3);
            self.save_now();
            match self.probe_metric(3, 1000, profiler.as_deref_mut()) {
                Ok(metric) => {
                    metric3 = metric;
                    if self.options.get("displayMode") == Some(3) {
                        r.add_flags(AutoSetupResult::FLAG_TOOLKIT3_OK);
                        let old = self
                            .toolkit3_driver
                            .and_then(|(vendor, version)| old_driver_flag(vendor, version));
                        if let Some(flag) = old {
                            r.add_flags(flag);
                            probe_toolkit1 = false;
                        }
                    }
                }
                Err(ProbeFailed) => r.add_flags(AutoSetupResult::FLAG_TOOLKIT3_FAILED),
            }
        }
        if probe_toolkit5 {
            self.options.set_field("safeMode", 5);
            self.save_now();
            match self.probe_metric(5, 1000, profiler.as_deref_mut()) {
                Ok(metric) => {
                    metric5 = metric;
                    if self.options.get("displayMode") == Some(5) {
                        r.add_flags(AutoSetupResult::FLAG_TOOLKIT5_OK);
                    }
                }
                Err(ProbeFailed) => r.add_flags(AutoSetupResult::FLAG_TOOLKIT5_FAILED),
            }
        }
        if probe_toolkit1 {
            self.options.set_field("safeMode", 4);
            self.save_now();
            match self.probe_metric(1, 1000, profiler) {
                Ok(metric) => {
                    metric1 = metric;
                    if self.options.get("displayMode") == Some(1) {
                        r.add_flags(AutoSetupResult::FLAG_TOOLKIT1_OK);
                    }
                }
                Err(ProbeFailed) => r.add_flags(AutoSetupResult::FLAG_TOOLKIT1_FAILED),
            }
        }
        self.options.set_field("safeMode", 0);
        if metric1 == -1 && metric3 == -1 {
            self.apply_unmeasured_setup(&mut r);
            return r;
        }
        r.record_metric(ProbeSlot::Toolkit3, metric3);
        r.record_metric(ProbeSlot::Toolkit1, metric1);
        r.record_metric(ProbeSlot::Toolkit5, metric5);
        // Toolkit 3 is favoured by a 30% margin.
        let weighted_metric3 = (metric3 as f32 * 1.3) as i32;
        if (weighted_metric3 <= 100000 || metric1 <= 100000) && weighted_metric3 <= metric1 {
            let metric = if weighted_metric3 == -1 {
                metric1
            } else {
                weighted_metric3
            };
            self.apply_measured_setup(&mut r, 1, metric);
        } else {
            let metric = if metric1 == -1 {
                weighted_metric3
            } else {
                metric1
            };
            self.apply_measured_setup(&mut r, 3, metric);
        }
        r
    }

    /// The setup used when no toolkit could be measured: the software
    /// toolkit with a preset chosen from the CPU probe (or the lowest preset
    /// on a small heap).
    fn apply_unmeasured_setup(&mut self, r: &mut AutoSetupResult) {
        r.set_chosen_toolkit(0);
        let level = if self.options.profile.max_memory_mb >= 96 {
            let cpu = cpu_profile();
            let level = crate::client_options::ClientOptions::autosetup_preset_for_cpu_profile(
                cpu,
                self.options.profile.max_memory_mb,
            );
            r.record_metric(ProbeSlot::CpuScore, cpu);
            level
        } else {
            r.add_flags(AutoSetupResult::FLAG_LOW_MEMORY);
            1
        };
        self.apply_preset_level(level);
        if self.options.get("displayMode") == Some(0) {
            self.options.set_display_flag("displayMode", true);
        } else {
            self.options.set_field("toolkit", 0);
            self.set_toolkit(0, false);
        }
        self.save_now();
        r.result = level;
    }

    /// The setup for a measured toolkit: the preset its metric selects,
    /// on that toolkit.
    fn apply_measured_setup(&mut self, r: &mut AutoSetupResult, toolkit: i32, metric: i32) {
        r.set_chosen_toolkit(toolkit);
        let level = crate::client_options::ClientOptions::autosetup_preset_for_metric(metric);
        self.apply_preset_level(level);
        if self.options.get("displayMode") == Some(toolkit) {
            self.options.set_display_flag("displayMode", true);
        } else {
            self.options.set_field("toolkit", toolkit);
            self.set_toolkit(toolkit, false);
        }
        self.save_now();
        r.result = level;
    }

    /// Applies a quality preset level (high, medium, low or minimum) and its
    /// consumer tails: the toolkit's bloom is switched off, the model caches
    /// are reset and the environment fade restarts.
    pub fn apply_preset_level(&mut self, level: i32) {
        self.options.apply_preset(level);
        if self.bloom && self.bloom_enabled {
            // The toolkit's bloom is disabled; the preference is already 0.
            self.bloom_enabled = false;
        }
        self.pending_effects
            .push(PreferenceEffect::ResetModelCaches);
        self.pending_effects
            .push(PreferenceEffect::ResetEnvironmentFade);
        self.window.changed = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_setup_result_packet_layout() {
        let mut r = AutoSetupResult::default();
        r.add_flags(AutoSetupResult::FLAG_TOOLKIT1_OK | AutoSetupResult::FLAG_TOOLKIT5_OK);
        r.set_chosen_toolkit(1);
        r.record_metric(ProbeSlot::Toolkit1, 1);
        r.record_metric(ProbeSlot::Toolkit5, 0x123456);
        r.record_metric(ProbeSlot::CpuScore, 99999);
        r.result = 1;
        let bytes = r.encode();
        assert_eq!(bytes.len(), 19);
        assert_eq!(
            bytes,
            vec![
                112, 0x20, 0x02, 129, 0xff, 0xff, 0xff, 0x34, 0x12, 0x56, 0x7f, 0xff, 127, 0xff,
                0xff, 0xff, 0, 1, 0
            ]
        );
    }
}
