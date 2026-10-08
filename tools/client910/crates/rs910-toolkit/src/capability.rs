//! The capability half of the toolkit (`docs/renderer/modern-renderer.md`):
//! what a hardware
//! toolkit answers the UI, CS2 and `ClientOptions` about the device.
//! Anti-aliasing support, bloom support and the scene sample
//! levels a toolkit accepts decide the saved `antiAliasing`/
//! `bloom` options, the graphics settings the scripts show and the anti-
//! aliasing fallback; the GL compressed
//! texture formats go into the input telemetry.
//!
//! A [`Profile`] is those answers as data. The faithful GPU toolkit builds
//! it from its device (`rs910_render_gpu::render::Renderer::capability_profile`);
//! the shell's renderer choice (`client910::active_toolkit::RendererKind`)
//! decides which profile every hardware-toolkit answer comes from. Every
//! backend that stands in for a hardware toolkit (the null backend, the
//! modern renderer) answers with the faithful profile, so choosing a
//! renderer never changes packets, per-tick state, CS2 results or
//! `ClientOptions` (programme §1 invariants 1-4).
//!
//! [`Answers`] adds the active toolkit to the profile. Toolkit 0
//! (`displayMode` 0, the software toolkit) is game state: it answers no bloom
//! and no anti-aliasing whatever draws its frames (a GPU renderer since the
//! software toolkit was removed), so a toolkit-0 session saves and sends
//! what it did when a software rasteriser drew it.
//!
//! Not in the profile: the per-frame post-process capture
//! (state of the live effect chain) and the
//! performance-metric benchmark (it creates its own profiling toolkit; its
//! device half is `rs910_gpu_device::ui_preferences_metric_gpu`).
use std::collections::BTreeSet;

/// See the module docs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Profile {
    /// Scene multisample counts the device supports (the MSAA
    /// levels).
    pub scene_samples: BTreeSet<u32>,
    /// Sample counts the HDR (bloom) scene target supports.
    pub hdr_samples: BTreeSet<u32>,
    /// The `GL_COMPRESSED_TEXTURE_FORMATS` codes
    /// (`compressed_texture_format`), in the device's report order.
    pub compressed_texture_formats: Vec<i32>,
}

impl Profile {
    /// Whether a toolkit with `count` scene samples can be created.
    pub fn supports_scene_samples(&self, count: u32) -> bool {
        self.scene_samples.contains(&count)
    }
    /// Whether bloom is supported on this device.
    pub fn supports_bloom(&self) -> bool {
        self.hdr_samples.contains(&1)
    }
    /// Whether anti-aliasing is supported on this device.
    pub fn supports_antialiasing(&self) -> bool {
        self.supports_scene_samples(2) && self.supports_scene_samples(4)
    }
}

/// The capability answers of the active toolkit (see the module docs):
/// the hardware [`Profile`] and whether toolkit 0 is the active toolkit.
/// The shell's `ActiveToolkit` answers through it, and so does the headless
/// session replay.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Answers {
    /// Toolkit 0 (`displayMode` 0, the software toolkit) is the active toolkit.
    pub toolkit0: bool,
    pub profile: Profile,
}

impl Answers {
    /// The hardware toolkit's answers (toolkits 1/5).
    pub fn hardware(profile: Profile) -> Self {
        Self {
            toolkit0: false,
            profile,
        }
    }
    /// `Toolkit.isBloomSupported`: false for toolkit 0
    /// (the pure-software toolkit has none), else the profile's.
    pub fn supports_bloom(&self) -> bool {
        !self.toolkit0 && self.profile.supports_bloom()
    }
    /// `Toolkit.supportsAntiAliasing`: false for toolkit 0
    /// (the pure-software toolkit has none), else the profile's.
    pub fn supports_antialiasing(&self) -> bool {
        !self.toolkit0 && self.profile.supports_antialiasing()
    }
    /// Whether a `Toolkit.create` with `count` scene samples succeeds (the
    /// profile's, for every toolkit).
    pub fn supports_scene_samples(&self, count: u32) -> bool {
        self.profile.supports_scene_samples(count)
    }
    /// The GL codes of the compressed texture formats, for the telemetry
    /// report (the profile's, for every toolkit).
    pub fn compressed_texture_formats(&self) -> &[i32] {
        &self.profile.compressed_texture_formats
    }
}
