//! The subsystems' GPU halves: their `impl ModernRenderer` blocks,
//! pipelines and GPU state (their CPU halves are the subsystem modules;
//! keeping these here keeps the module graph acyclic).

pub(crate) mod ambient;
pub(crate) mod atmosphere;
pub(crate) mod caustics;
pub(crate) mod far;
pub(crate) mod global_env_mapping;
pub(crate) mod interior;
pub(crate) mod light_probes;
pub(crate) mod planar_reflection;
pub(crate) mod point_lights;
pub(crate) mod point_shadows;
pub(crate) mod post;
pub(crate) mod shadow_cache;
pub(crate) mod sky_box;
pub(crate) mod sky_layers;
pub(crate) mod sprites;
pub(crate) mod sunlight_shadows;
pub(crate) mod terrain;
pub(crate) mod terrain_upload;
pub(crate) mod water;
