//! The renderer's WGSL, composed from ordered source parts.
//!
//! Every shader module is a fixed, ordered list of `.wgsl` snippets that
//! live with their subsystem (`frame/`, `models/`, `lighting/`, `shadows/`,
//! `atmosphere/`, `post/`, `water_body/`, `terrain/`, `sprites/`), plus the
//! few constants generated from their Rust sources of truth. A module's
//! text is composed once per [`Module`] (its variant included) and
//! compiled once per renderer ([`Library`]); no module is edited after it
//! is composed.
//!
//! The WGSL is this crate's own implementation of the modern client's shading
//! (`docs/renderer/modern-renderer.md`, "Sources of truth").
//!
//! Colour space: display-referred classic colours are decoded with a 2.2
//! power (`display_to_linear`, as the modern client decodes its environment
//! colours); sRGB texture samples are re-expressed in that transfer
//! (`texture_linear`).
//! Space: every position is camera-local classic fine units (y down), the
//! camera target subtracted on the CPU, so large world coordinates never
//! reach the GPU.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::settings::LookMode;

/// The display transfer, the sRGB curves and the calibrated tonemap with
/// its inverse (the classic fog and sky colours are display-referred and enter
/// the HDR target through it; `post::tonemap` is the CPU half).
pub const COMMON: &str = include_str!("common.wgsl");

const FRAME: &str = include_str!("../frame/frame.wgsl");
const FORWARD_IO: &str = include_str!("../frame/forward_io.wgsl");
const FOG: &str = include_str!("../atmosphere/fog.wgsl");
const GEOMETRY: &str = include_str!("../models/geometry.wgsl");
const SURFACE: &str = include_str!("../models/surface.wgsl");
const POINT_LIGHTS: &str = include_str!("../lighting/point_lights.wgsl");
const SUN_SHADOWS: &str = include_str!("../shadows/sun.wgsl");
const SHADING: &str = include_str!("../models/shading.wgsl");
const SPRITES: &str = include_str!("../sprites/sprites.wgsl");
const POINT_SHADOWS: &str = include_str!("../shadows/point.wgsl");
const PROBES: &str = include_str!("../lighting/probes.wgsl");
const AMBIENT: &str = include_str!("../lighting/ambient.wgsl");
const SCATTERING: &str = include_str!("../atmosphere/scattering.wgsl");
const ENV_REFLECTIONS: &str = include_str!("../lighting/env_reflections.wgsl");
const AO_GEOMETRY: &str = include_str!("../post/geometry.wgsl");
const TERRAIN: &str = include_str!("../terrain/terrain.wgsl");
const TERRAIN_CAUSTICS: &str = include_str!("../terrain/caustics.wgsl");
const WATER: &str = include_str!("../water_body/water.wgsl");
const WATER_DEPTH_SINGLE: &str = include_str!("../water_body/depth_single.wgsl");
const WATER_DEPTH_MULTI: &str = include_str!("../water_body/depth_multi.wgsl");
const WATER_EFFECTS: &str = include_str!("../water_body/effects.wgsl");
const WATER_ENV: &str = include_str!("../water_body/env.wgsl");
const WATER_EXTINCTION: &str = include_str!("../water_body/extinction.wgsl");
const WATER_CAUSTIC_RAYS: &str = include_str!("../water_body/caustic_rays.wgsl");
const CAUSTICS_RESOLVE: &str = include_str!("../water_body/caustics_resolve.wgsl");
const ATMOSPHERE_PASSES: &str = include_str!("../atmosphere/passes.wgsl");
const ATMOSPHERE_DEPTH_SINGLE: &str = include_str!("../atmosphere/depth_single.wgsl");
const ATMOSPHERE_DEPTH_MULTI: &str = include_str!("../atmosphere/depth_multi.wgsl");
const SKY: &str = include_str!("../atmosphere/sky.wgsl");
const VOLUMETRICS: &str = include_str!("../atmosphere/volumetrics.wgsl");
const DOF: &str = include_str!("../post/dof.wgsl");
const SKY_LAYERS: &str = include_str!("../atmosphere/sky_layers.wgsl");
const POST: &str = include_str!("../post/post.wgsl");
const TONE_MAP_CALIBRATED: &str = include_str!("../post/tone_map_calibrated.wgsl");
const TONE_MAP_VERIFIED: &str = include_str!("../post/tone_map_verified.wgsl");
const AO: &str = include_str!("../post/ao.wgsl");
const UPSCALE: &str = include_str!("../post/upscale.wgsl");
const PROBE_FACE: &str = include_str!("../lighting/probe_face.wgsl");
const PROBE_FILTER: &str = include_str!("../lighting/probe_filter.wgsl");
const PROBE_PROJECT: &str = include_str!("../lighting/probe_project.wgsl");
const SHADOW_FILL: &str = include_str!("../shadows/fill.wgsl");

/// One shader module the renderer compiles, with its variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Module {
    /// The forward library and its passes: the lit models, floors and sky
    /// models, the depth pre-pass, the sun and point-light casters, the
    /// sprites, the probe capture and the water reflection.
    Forward,
    /// The ambient occlusion's geometry pass (normals and positions).
    AoGeometry,
    Terrain,
    /// The water surface and the caustic rays (the scene depth's binding
    /// type follows the forward target's sample count).
    Water {
        multisampled: bool,
    },
    /// The sky, volumetric scattering and depth of field passes.
    Atmosphere {
        multisampled: bool,
    },
    /// The classic skybox layers.
    SkyLayers,
    /// The post chain (the composite's operator follows the look).
    Post {
        look: LookMode,
    },
    ProbeFilters,
    ProbeProjection,
    CausticsResolve,
    /// The shadow maps' tile clears and restores (`shadows::cache`).
    ShadowFill,
}

impl Module {
    /// The wgpu label.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Forward => "modern forward",
            Self::AoGeometry => "modern geometry",
            Self::Terrain => "modern terrain",
            Self::Water { .. } => "modern water",
            Self::Atmosphere { .. } => "modern atmosphere",
            Self::SkyLayers => "modern sky",
            Self::Post { .. } => "modern post",
            Self::ProbeFilters => "modern probe filters",
            Self::ProbeProjection => "modern probe projection",
            Self::CausticsResolve => "modern caustics resolve",
            Self::ShadowFill => "modern shadow fill",
        }
    }

    /// The module's ordered source parts.
    fn parts(self) -> Vec<std::borrow::Cow<'static, str>> {
        use std::borrow::Cow::{Borrowed as B, Owned as O};
        let forward = || {
            vec![
                B(COMMON),
                B(FRAME),
                B(FORWARD_IO),
                B(FOG),
                B(GEOMETRY),
                B(SURFACE),
                B(POINT_LIGHTS),
                B(SUN_SHADOWS),
                O(crate::models::shading::constants_wgsl()),
                B(SHADING),
                B(SPRITES),
                B(POINT_SHADOWS),
                B(PROBES),
                B(AMBIENT),
                B(SCATTERING),
                B(ENV_REFLECTIONS),
            ]
        };
        match self {
            Self::Forward => forward(),
            Self::AoGeometry => [forward(), vec![B(AO_GEOMETRY)]].concat(),
            Self::Terrain => [forward(), vec![B(TERRAIN), B(TERRAIN_CAUSTICS)]].concat(),
            Self::Water { multisampled } => {
                let depth = if multisampled {
                    WATER_DEPTH_MULTI
                } else {
                    WATER_DEPTH_SINGLE
                };
                let water = [
                    WATER,
                    depth,
                    WATER_EFFECTS,
                    WATER_ENV,
                    WATER_EXTINCTION,
                    WATER_CAUSTIC_RAYS,
                ];
                [forward(), water.map(B).to_vec()].concat()
            }
            Self::Atmosphere { multisampled } => {
                let depth = if multisampled {
                    ATMOSPHERE_DEPTH_MULTI
                } else {
                    ATMOSPHERE_DEPTH_SINGLE
                };
                [
                    COMMON,
                    FRAME,
                    SCATTERING,
                    ATMOSPHERE_PASSES,
                    depth,
                    SKY,
                    VOLUMETRICS,
                    DOF,
                ]
                .map(B)
                .to_vec()
            }
            Self::SkyLayers => vec![B(COMMON), B(SKY_LAYERS)],
            Self::Post { look } => vec![
                B(COMMON),
                B(POST),
                B(match look {
                    LookMode::ClassicCalibrated => TONE_MAP_CALIBRATED,
                    LookMode::Verified => TONE_MAP_VERIFIED,
                }),
                B(AO),
                B(UPSCALE),
            ],
            Self::ProbeFilters => vec![B(PROBE_FACE), B(PROBE_FILTER)],
            Self::ProbeProjection => vec![B(COMMON), B(PROBE_FACE), B(PROBE_PROJECT)],
            Self::CausticsResolve => vec![B(CAUSTICS_RESOLVE)],
            Self::ShadowFill => vec![B(SHADOW_FILL)],
        }
    }

    /// The module's WGSL.
    #[must_use]
    pub fn source(self) -> String {
        self.parts().concat()
    }
}

/// The renderer's compiled shader modules: each [`Module`] composed and
/// compiled once, on first use.
#[derive(Default)]
pub struct Library {
    modules: Mutex<HashMap<Module, Arc<wgpu::ShaderModule>>>,
}

impl Library {
    /// `module`, compiled on first use.
    pub fn get(&self, device: &wgpu::Device, module: Module) -> Arc<wgpu::ShaderModule> {
        self.modules
            .lock()
            .expect("the shader library")
            .entry(module)
            .or_insert_with(|| {
                Arc::new(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                    label: Some(module.label()),
                    source: wgpu::ShaderSource::Wgsl(module.source().into()),
                }))
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every module, each variant, composes to WGSL that parses and
    /// validates (a snippet left out or out of step fails here, before any
    /// device compiles it).
    #[test]
    fn every_module_composes_to_valid_wgsl() {
        let mut modules = vec![
            Module::Forward,
            Module::AoGeometry,
            Module::Terrain,
            Module::SkyLayers,
            Module::ProbeFilters,
            Module::ProbeProjection,
            Module::CausticsResolve,
            Module::ShadowFill,
        ];
        for multisampled in [false, true] {
            modules.push(Module::Water { multisampled });
            modules.push(Module::Atmosphere { multisampled });
        }
        for look in [LookMode::ClassicCalibrated, LookMode::Verified] {
            modules.push(Module::Post { look });
        }
        for m in modules {
            let source = m.source();
            let module = wgpu::naga::front::wgsl::parse_str(&source)
                .unwrap_or_else(|e| panic!("{m:?}: {}", e.emit_to_string(&source)));
            wgpu::naga::valid::Validator::new(
                wgpu::naga::valid::ValidationFlags::all(),
                wgpu::naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|e| panic!("{m:?}: {e:?}"));
        }
    }
}
