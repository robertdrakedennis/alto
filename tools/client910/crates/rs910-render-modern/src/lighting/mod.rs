//! Lighting: the environment's sun and ambient ([`environment`]) and its
//! record's colour remap ([`environment_record`]), the calibrated look
//! ([`look`]), static point lights ([`point_lights`]), the per-square ambient
//! irradiance captured from the world ([`ambient`], [`ambient_schedule`]),
//! light probes and image-based lighting ([`probes`]) and RT5 environment
//! mapping ([`env_reflections`]). Their GPU halves are in `frame::gpu`.

pub mod ambient;
pub mod ambient_schedule;
pub mod env_reflections;
pub mod environment;
pub mod environment_record;
pub mod look;
pub mod point_lights;
pub mod probes;
