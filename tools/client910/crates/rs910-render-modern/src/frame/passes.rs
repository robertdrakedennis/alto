//! The frame graph's declarations: every pass the frame encodes, in frame
//! order, with its resolution class, sample count, what it reads and
//! writes, and where a multisampled target resolves. `ModernRenderer::encode`
//! and the subsystems' `encode_*` begin each pass through
//! [`ModernRenderer::begin_pass`], which names it from here and (in debug
//! builds) checks the frame keeps the declared stage order.
//!
//! Modelled on the modern client's view passes (the passes run in push
//! order; the forward target takes the device's MSAA count). The
//! declarations are the boundaries later performance work builds on
//! (transient target aliasing, one resolve at the boundary that reads it,
//! per-pass encoders and timestamps; `tools/perf/modern-performance-plan.md` §8).
//!
//! The passes record in encode units (`frame::units`), each into its own
//! command encoder on a worker thread (`frame::jobs`), submitted in frame
//! order.
//!
//! Bind groups follow update frequency: group 0 is the frame block (and the
//! AO map), written once per frame; group 1 the material (few per frame);
//! groups 2 and 3 the pass inputs (the shadow atlas and its receive block,
//! the point lights, probes); per-draw data rides the instance stream.

use crate::frame::encoding::EncodeInputs;
use std::cell::Cell;

/// A pass of the frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    ProbeFilter,
    ProbeSky,
    ProbeCapture,
    ProbeProjection,
    EnvironmentCapture,
    AmbientCapture,
    Sky,
    SkyShading,
    SunShadowsStatic,
    SunShadows,
    PointShadows,
    CausticRays,
    CausticsResolve,
    DepthPrepass,
    AoGeometry,
    Ao,
    AoBlurX,
    AoBlurY,
    WaterReflection,
    Forward,
    ForwardBeforeWater,
    Water,
    ForwardAfterWater,
    VolumetricsDepth,
    VolumetricsMarch,
    Volumetrics,
    DofFocus,
    DofSpread,
    DofBlur,
    DofComposite,
    Luminance,
    Adaptation,
    BrightPass,
    Bloom,
    Composite,
    Fxaa,
    Upscale,
}

/// The frame's stages: passes run in stage order; within a stage a pass
/// may repeat (the probe faces, the blur chains).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// A light-probe or environment capture (when the scene or the
    /// settled environment changes).
    Probes,
    /// The sky is the frame's background: drawn before the shadow passes and the geometry.
    Sky,
    Shadows,
    Caustics,
    Depth,
    AmbientOcclusion,
    Forward,
    Atmosphere,
    Post,
}

/// A pass's render size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resolution {
    /// The scene's size: the frame's target, or at a render scale below 1
    /// the scaled scene viewport (`frame::scale`).
    Full,
    /// The scene's size over a divisor (the bloom's `BLOOM_DOWNSAMPLE`, the
    /// volumetrics' half size, the water reflection's
    /// `ModernSettings::reflections`).
    Scaled,
    /// A fixed size (the shadow atlas and cube faces by quality, the
    /// caustic map, the luminance chain, the probe cubes).
    Fixed,
    /// The shell's frame (the drawable), whatever the render scale.
    Output,
}

/// A pass's sample count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Samples {
    /// The forward target's (the client's anti-aliasing level).
    Forward,
    One,
}

/// A frame resource a pass reads or writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Resource {
    ProbeCubes,
    EnvironmentCube,
    ShadowAtlas,
    /// The sun cascades' static casters (`shadows::cache`).
    ShadowStatic,
    PointShadowMaps,
    CausticMap,
    /// The forward colour target (multisampled at the forward count).
    Hdr,
    /// Its single-sample resolve, which everything after the forward
    /// stage reads (the post chain: whichever of it and [`Self::HdrTwin`]
    /// the atmosphere's full-frame passes wrote last).
    HdrResolved,
    Depth,
    /// The classic sky's resolve, the sky shading's source.
    SkySource,
    /// The resolve's twin: the atmosphere's full-frame passes read one and
    /// write the other (`Targets::hdr_views`).
    HdrTwin,
    /// The scene copy the water looks through (`RefractionRT`): group 0's
    /// resolve, or its copy at one sample.
    Refraction,
    AoGeometry,
    AoMap,
    /// The volumetrics' half-size depth and scattering.
    VolumetricsHalf,
    WaterReflection,
    Luminance,
    BloomChain,
    DofChain,
    Ldr,
    /// The frame's colour target (the shell's).
    Output,
}

/// One pass's declaration.
#[derive(Clone, Copy, Debug)]
pub struct Decl {
    pub pass: Pass,
    pub label: &'static str,
    pub stage: Stage,
    pub resolution: Resolution,
    pub samples: Samples,
    pub reads: &'static [Resource],
    pub writes: &'static [Resource],
    /// The single-sample target a multisampled write resolves into at the
    /// end of the pass.
    pub resolves: Option<Resource>,
}

/// A table row: the pass, its label and stage, its (resolution, samples),
/// what it reads and writes, and its resolve.
const fn decl(
    pass: Pass,
    label: &'static str,
    stage: Stage,
    (resolution, samples): (Resolution, Samples),
    reads: &'static [Resource],
    writes: &'static [Resource],
    resolves: Option<Resource>,
) -> Decl {
    Decl {
        pass,
        label,
        stage,
        resolution,
        samples,
        reads,
        writes,
        resolves,
    }
}

use Resolution::{Fixed, Full, Output as Drawable, Scaled};
use Resource as R;
use Samples::{Forward as Msaa, One};
use Stage as S;

/// The frame's passes in frame order.
pub const FRAME: [Decl; 37] = [
    decl(
        Pass::ProbeFilter,
        "modern probe filter",
        S::Probes,
        (Fixed, One),
        &[R::EnvironmentCube],
        &[R::EnvironmentCube],
        None,
    ),
    decl(
        Pass::ProbeSky,
        "modern probe sky",
        S::Probes,
        (Fixed, One),
        &[],
        &[R::ProbeCubes],
        None,
    ),
    decl(
        Pass::ProbeCapture,
        "modern probe capture",
        S::Probes,
        (Fixed, One),
        &[R::ShadowAtlas],
        &[R::ProbeCubes],
        None,
    ),
    decl(
        Pass::ProbeProjection,
        "modern probe projection",
        S::Probes,
        (Fixed, One),
        &[R::ProbeCubes],
        &[R::ProbeCubes],
        None,
    ),
    decl(
        Pass::EnvironmentCapture,
        "modern environment capture",
        S::Probes,
        (Fixed, One),
        &[],
        &[R::EnvironmentCube],
        None,
    ),
    decl(
        Pass::AmbientCapture,
        "modern ambient capture",
        S::Probes,
        (Fixed, One),
        &[],
        &[R::ProbeCubes],
        None,
    ),
    decl(
        Pass::Sky,
        "modern sky",
        S::Sky,
        (Full, Msaa),
        &[],
        &[R::Hdr],
        Some(R::SkySource),
    ),
    decl(
        Pass::SkyShading,
        "modern sky shading",
        S::Sky,
        (Full, One),
        &[R::SkySource],
        &[R::HdrResolved],
        None,
    ),
    decl(
        Pass::SunShadowsStatic,
        "modern sun shadows (static)",
        S::Shadows,
        (Fixed, One),
        &[],
        &[R::ShadowStatic],
        None,
    ),
    decl(
        Pass::SunShadows,
        "modern sun shadows",
        S::Shadows,
        (Fixed, One),
        &[R::ShadowStatic],
        &[R::ShadowAtlas],
        None,
    ),
    decl(
        Pass::PointShadows,
        "modern point shadows",
        S::Shadows,
        (Fixed, One),
        &[],
        &[R::PointShadowMaps],
        None,
    ),
    decl(
        Pass::CausticRays,
        "modern caustic rays",
        S::Caustics,
        (Fixed, One),
        &[],
        &[R::CausticMap],
        None,
    ),
    decl(
        Pass::CausticsResolve,
        "modern caustics resolve",
        S::Caustics,
        (Fixed, One),
        &[R::CausticMap],
        &[R::CausticMap],
        None,
    ),
    decl(
        Pass::DepthPrepass,
        "modern depth pre-pass",
        S::Depth,
        (Full, Msaa),
        &[],
        &[R::Depth],
        None,
    ),
    decl(
        Pass::AoGeometry,
        "modern geometry",
        S::AmbientOcclusion,
        (Scaled, One),
        &[],
        &[R::AoGeometry],
        None,
    ),
    decl(
        Pass::Ao,
        "modern ao",
        S::AmbientOcclusion,
        (Scaled, One),
        &[R::AoGeometry],
        &[R::AoMap],
        None,
    ),
    decl(
        Pass::AoBlurX,
        "modern ao blur x",
        S::AmbientOcclusion,
        (Scaled, One),
        &[R::AoMap],
        &[R::AoMap],
        None,
    ),
    decl(
        Pass::AoBlurY,
        "modern ao blur y",
        S::AmbientOcclusion,
        (Scaled, One),
        &[R::AoMap],
        &[R::AoMap],
        None,
    ),
    decl(
        Pass::WaterReflection,
        "modern water reflection",
        S::Forward,
        (Scaled, One),
        &[R::ShadowAtlas, R::CausticMap],
        &[R::WaterReflection],
        None,
    ),
    decl(
        Pass::Forward,
        "modern forward lighting",
        S::Forward,
        (Full, Msaa),
        &[
            R::ShadowAtlas,
            R::AoMap,
            R::CausticMap,
            R::ProbeCubes,
            R::EnvironmentCube,
        ],
        &[R::Hdr, R::Depth],
        Some(R::HdrResolved),
    ),
    decl(
        Pass::ForwardBeforeWater,
        "modern forward lighting (group 0)",
        S::Forward,
        (Full, Msaa),
        &[
            R::ShadowAtlas,
            R::AoMap,
            R::CausticMap,
            R::ProbeCubes,
            R::EnvironmentCube,
        ],
        &[R::Hdr, R::Depth],
        Some(R::Refraction),
    ),
    decl(
        Pass::Water,
        "modern water",
        S::Forward,
        (Full, Msaa),
        &[
            R::Depth,
            R::Refraction,
            R::WaterReflection,
            R::ShadowAtlas,
            R::EnvironmentCube,
        ],
        &[R::Hdr],
        None,
    ),
    decl(
        Pass::ForwardAfterWater,
        "modern forward lighting (group 2)",
        S::Forward,
        (Full, Msaa),
        &[R::ShadowAtlas, R::AoMap, R::ProbeCubes, R::EnvironmentCube],
        &[R::Hdr, R::Depth],
        Some(R::HdrResolved),
    ),
    // The half-size volumetrics (`atmosphere::volumetrics`): the
    // farthest depth of each 2x2, the march, the bilateral apply.
    decl(
        Pass::VolumetricsDepth,
        "modern volumetrics depth",
        S::Atmosphere,
        (Scaled, One),
        &[R::Depth],
        &[R::VolumetricsHalf],
        None,
    ),
    decl(
        Pass::VolumetricsMarch,
        "modern volumetrics march",
        S::Atmosphere,
        (Scaled, One),
        &[R::VolumetricsHalf, R::ShadowAtlas],
        &[R::VolumetricsHalf],
        None,
    ),
    decl(
        Pass::Volumetrics,
        "modern volumetrics",
        S::Atmosphere,
        (Full, One),
        &[R::HdrResolved, R::Depth, R::VolumetricsHalf],
        &[R::HdrTwin],
        None,
    ),
    decl(
        Pass::DofFocus,
        "modern dof focus",
        S::Atmosphere,
        (Full, One),
        &[R::Depth],
        &[R::DofChain],
        None,
    ),
    decl(
        Pass::DofSpread,
        "modern dof spread",
        S::Atmosphere,
        (Full, One),
        &[R::DofChain],
        &[R::DofChain],
        None,
    ),
    decl(
        Pass::DofBlur,
        "modern dof blur",
        S::Atmosphere,
        (Full, One),
        &[R::HdrResolved, R::HdrTwin, R::DofChain],
        &[R::DofChain],
        None,
    ),
    decl(
        Pass::DofComposite,
        "modern dof composite",
        S::Atmosphere,
        (Full, One),
        &[R::HdrResolved, R::HdrTwin, R::DofChain],
        &[R::HdrTwin],
        None,
    ),
    decl(
        Pass::Luminance,
        "modern luminance",
        S::Post,
        (Fixed, One),
        &[R::HdrResolved, R::HdrTwin],
        &[R::Luminance],
        None,
    ),
    decl(
        Pass::Adaptation,
        "modern adaptation",
        S::Post,
        (Fixed, One),
        &[R::Luminance],
        &[R::Luminance],
        None,
    ),
    decl(
        Pass::BrightPass,
        "modern bright pass",
        S::Post,
        (Scaled, One),
        &[R::HdrResolved, R::HdrTwin, R::Luminance],
        &[R::BloomChain],
        None,
    ),
    decl(
        Pass::Bloom,
        "modern kawase",
        S::Post,
        (Scaled, One),
        &[R::BloomChain],
        &[R::BloomChain],
        None,
    ),
    decl(
        Pass::Composite,
        "modern composite",
        S::Post,
        (Full, One),
        &[R::HdrResolved, R::HdrTwin, R::Luminance, R::BloomChain],
        &[R::Output, R::Ldr],
        None,
    ),
    decl(
        Pass::Fxaa,
        "modern fxaa",
        S::Post,
        (Full, One),
        &[R::Ldr],
        &[R::Output, R::Ldr],
        None,
    ),
    // The render scale's upscale (`frame::scale`; below 100% only).
    decl(
        Pass::Upscale,
        "modern upscale",
        S::Post,
        (Drawable, One),
        &[R::Ldr],
        &[R::Output],
        None,
    ),
];

/// Resources that share one texture (lane P4-GPU: transient aliasing):
/// their lifetimes in [`FRAME`] (from the first pass that writes or resolves
/// into one to the last that reads it) do not overlap, so each group is one
/// full-frame HDR texture, the frame's resolve twin
/// (`frame::Targets::scratch`): the classic sky's resolve (sky to sky shading),
/// the water's scene copy (forward group 0 to the water) and the
/// atmosphere's output (volumetrics or depth of field to the post chain).
/// (Also, outside the table: the classic sky draws into the forward target,
/// which the sky shading clears; the occlusion before its blur shares the
/// occlusion map's texture, `frame::gpu::post`.)
pub const ALIASES: &[&[Resource]] = &[&[R::SkySource, R::Refraction, R::HdrTwin]];

/// The passes (indices into [`FRAME`]) from the first that writes or
/// resolves into `r` to the last that reads it (`None`: never written).
#[must_use]
pub fn lifetime(r: Resource) -> Option<(usize, usize)> {
    let first = FRAME
        .iter()
        .position(|d| d.writes.contains(&r) || d.resolves == Some(r))?;
    let last = FRAME
        .iter()
        .rposition(|d| d.reads.contains(&r))
        .unwrap_or(first);
    Some((first, last.max(first)))
}

impl Pass {
    /// The pass's declaration.
    #[must_use]
    pub fn decl(self) -> &'static Decl {
        FRAME
            .iter()
            .find(|d| d.pass == self)
            .expect("every pass is declared")
    }

    /// The pass's wgpu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        self.decl().label
    }
}

/// The encode's pass order so far, per thread (each encode unit records on
/// one thread, `frame::units`): debug builds check that every pass begins
/// in the unit the thread is encoding and at or after the stage of the
/// unit's previous pass. The units' own order is the submission order
/// (`frame::units::UNITS`, checked when a renderer is made).
#[derive(Debug, Default)]
pub(crate) struct Order;

thread_local! {
    /// The unit this thread encodes (`None`: the prepare) and the stage of
    /// its last pass.
    static CURRENT: Cell<(Option<crate::frame::units::Unit>, Option<Stage>)> =
        const { Cell::new((None, None)) };
}

impl Order {
    /// A new frame (`ModernRenderer::draw`, before the prepare phase, which
    /// may encode the global environment's projection).
    pub(crate) fn start(&self) {
        CURRENT.set((None, None));
    }

    /// This thread starts encoding `unit`.
    pub(crate) fn start_unit(&self, unit: crate::frame::units::Unit) {
        CURRENT.set((Some(unit), None));
    }

    pub(crate) fn begin(&self, pass: Pass) -> &'static str {
        let d = pass.decl();
        if cfg!(debug_assertions) {
            let (unit, prev) = CURRENT.get();
            if let Some(unit) = unit {
                assert!(
                    unit.decl().passes.contains(&pass),
                    "{pass:?} encoded in the {unit:?} unit"
                );
            }
            if let Some(prev) = prev {
                assert!(
                    d.stage >= prev,
                    "{pass:?} ({:?}) encoded after the {prev:?} stage",
                    d.stage
                );
            }
            CURRENT.set((unit, Some(d.stage)));
        }
        d.label
    }
}

impl<'a> EncodeInputs<'a> {
    /// Begin `pass` of this frame: its label (see the module docs).
    pub(crate) fn begin_pass(&self, pass: Pass) -> &'static str {
        self.pass_order.begin(pass)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each alias group's resources are live in turn, never together (a
    /// pass that read one of them after another is written would break the
    /// sharing).
    #[test]
    fn aliased_resources_are_never_live_together() {
        for group in ALIASES {
            let mut spans: Vec<(usize, usize, Resource)> = group
                .iter()
                .map(|&r| {
                    let (a, b) = lifetime(r).unwrap_or_else(|| panic!("{r:?} is never written"));
                    (a, b, r)
                })
                .collect();
            spans.sort_by_key(|s| s.0);
            for w in spans.windows(2) {
                assert!(
                    w[0].1 < w[1].0,
                    "{:?} (passes {}..={}) is still read when {:?} is written (pass {})",
                    w[0].2,
                    w[0].0,
                    w[0].1,
                    w[1].2,
                    w[1].0
                );
            }
        }
    }
}
