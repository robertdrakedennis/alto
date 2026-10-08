//! The subsystems' GPU halves: their `impl ModernRenderer` blocks,
//! pipelines and GPU state (their CPU halves are the subsystem modules;
//! keeping these here keeps the module graph acyclic).

pub(crate) mod ambient;
pub(crate) mod atmosphere;
pub(crate) mod caustics;
pub(crate) mod env_reflections;
pub(crate) mod far;
pub(crate) mod interior;
pub(crate) mod point_lights;
pub(crate) mod point_shadows;
pub(crate) mod post;
pub(crate) mod probes;
pub(crate) mod shadow_cache;
pub(crate) mod sky_cube;
pub(crate) mod sky_layers;
pub(crate) mod sprites;
pub(crate) mod sun_shadows;
pub(crate) mod terrain;
pub(crate) mod water;
pub(crate) mod water_reflection;
