//! The frame's encode units: the passes of [`crate::frame::passes::FRAME`]
//! grouped into runs that record independently. Each unit records into
//! its own command encoder, on whichever thread of `frame::jobs` claims it,
//! and the command buffers are submitted in the order below, so the GPU
//! runs the passes in the declared frame order whatever order the threads
//! finished in. The post chain, which writes the frame's own target,
//! records into the frame's encoder (the shell's, submitted after).
//!
//! A unit reads only what `ModernRenderer::draw` prepared (the draw list,
//! the frame packets, the uploads): encoding takes `&ModernRenderer`. A
//! subsystem's passes plug in as a unit: the shadows' cascades are
//! [`Unit::SunShadows`] (`ModernRenderer::encode_sun_shadows`, the shadow
//! caches' static tiles and dynamic casters, `shadows::cache`); the far
//! scene draws its packets inside the units that draw the scene.

use crate::frame::passes::{Pass, Stage};

/// See the module docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// A light-probe, environment or per-square ambient capture.
    Probes,
    /// The sky decor sprites and the sky shading: the frame's background.
    Sky,
    SunShadows,
    PointShadows,
    /// The water's caustics.
    Caustics,
    DepthPrepass,
    /// The geometry pass, the occlusion and its blur.
    AmbientOcclusion,
    WaterReflection,
    /// The forward lighting (with water: its group 0).
    Forward,
    /// With water: the water surfaces and the forward lighting's group 2.
    ForwardAfterWater,
    /// Volumetrics (half-size depth and march, the apply) and depth of
    /// field, then luminance, adaptation, bloom, the composite and FXAA into
    /// the frame, and at a render scale the upscale into it.
    Post,
}

/// One unit's declaration.
#[derive(Clone, Copy, Debug)]
pub struct UnitDecl {
    pub unit: Unit,
    pub label: &'static str,
    /// The passes it may encode, in frame order.
    pub passes: &'static [Pass],
    /// Whether it records into the frame's encoder (it writes the frame's
    /// target) rather than a command buffer of its own.
    pub frame_encoder: bool,
}

const fn unit(
    unit: Unit,
    label: &'static str,
    passes: &'static [Pass],
    frame_encoder: bool,
) -> UnitDecl {
    UnitDecl {
        unit,
        label,
        passes,
        frame_encoder,
    }
}

/// The units in submission order. The small passes share units, as a
/// command encoder of its own costs a unit about 170 allocations in wgpu.
pub const UNITS: [UnitDecl; 11] = [
    unit(
        Unit::Probes,
        "modern probes",
        &[
            Pass::ProbeSky,
            Pass::ProbeCapture,
            Pass::ProbeProjection,
            Pass::EnvironmentCapture,
            Pass::AmbientCapture,
        ],
        false,
    ),
    unit(
        Unit::Sky,
        "modern sky",
        &[Pass::Sky, Pass::SkyShading],
        false,
    ),
    unit(
        Unit::SunShadows,
        "modern sun shadows",
        &[Pass::SunShadowsStatic, Pass::SunShadows],
        false,
    ),
    unit(
        Unit::PointShadows,
        "modern point shadows",
        &[Pass::PointShadows],
        false,
    ),
    unit(
        Unit::Caustics,
        "modern caustics",
        &[Pass::CausticRays, Pass::CausticsResolve],
        false,
    ),
    unit(
        Unit::DepthPrepass,
        "modern depth pre-pass",
        &[Pass::DepthPrepass],
        false,
    ),
    unit(
        Unit::AmbientOcclusion,
        "modern ambient occlusion",
        &[Pass::AoGeometry, Pass::Ao, Pass::AoBlurX, Pass::AoBlurY],
        false,
    ),
    unit(
        Unit::WaterReflection,
        "modern water reflection",
        &[Pass::WaterReflection],
        false,
    ),
    unit(
        Unit::Forward,
        "modern forward",
        &[Pass::Forward, Pass::ForwardBeforeWater],
        false,
    ),
    unit(
        Unit::ForwardAfterWater,
        "modern water and forward group 2",
        &[Pass::Water, Pass::ForwardAfterWater],
        false,
    ),
    unit(
        Unit::Post,
        "modern atmosphere and post",
        &[
            Pass::VolumetricsDepth,
            Pass::VolumetricsMarch,
            Pass::Volumetrics,
            Pass::DofFocus,
            Pass::DofSpread,
            Pass::DofBlur,
            Pass::DofComposite,
            Pass::Luminance,
            Pass::Adaptation,
            Pass::BrightPass,
            Pass::Bloom,
            Pass::Composite,
            Pass::Fxaa,
            Pass::Upscale,
        ],
        true,
    ),
];

/// The order the threads take the units in: the longest first, so the
/// frame's encode ends soon after its longest unit (lane P7's unit times at
/// Lumbridge after the shadow caches: water reflection 1.7 ms, forward 1.2,
/// ambient occlusion 0.9, water and group 2 0.8, pre-pass 0.4, the rest
/// under 0.3). Only the threads' order: the submission order is
/// [`UNITS`]'.
pub const CLAIM_ORDER: [Unit; 11] = [
    Unit::WaterReflection,
    Unit::Forward,
    Unit::AmbientOcclusion,
    Unit::ForwardAfterWater,
    Unit::DepthPrepass,
    Unit::SunShadows,
    Unit::PointShadows,
    Unit::Probes,
    Unit::Post,
    Unit::Sky,
    Unit::Caustics,
];

impl Unit {
    /// The unit's declaration.
    #[must_use]
    pub fn decl(self) -> &'static UnitDecl {
        UNITS
            .iter()
            .find(|d| d.unit == self)
            .expect("every unit is declared")
    }

    /// Its place in the submission order.
    #[must_use]
    pub fn index(self) -> usize {
        UNITS
            .iter()
            .position(|d| d.unit == self)
            .expect("every unit is declared")
    }
}

/// Whether the units, in submission order, keep the frame's stage order
/// and hold every pass the encode begins (all but the probe filter, which
/// the prepare encodes) once (debug builds check it when a renderer is
/// made).
#[must_use]
pub(crate) fn declared_in_frame_order() -> bool {
    let stages = |d: &UnitDecl| {
        d.passes
            .iter()
            .map(|p| p.decl().stage)
            .collect::<Vec<Stage>>()
    };
    let ordered = UNITS
        .iter()
        .flat_map(stages)
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] <= w[1]);
    let once = crate::frame::passes::FRAME
        .iter()
        .filter(|d| d.pass != Pass::ProbeFilter)
        .all(|d| UNITS.iter().filter(|u| u.passes.contains(&d.pass)).count() == 1);
    let claimed = UNITS.iter().all(|d| CLAIM_ORDER.contains(&d.unit));
    ordered && once && claimed
}
