//! The modern renderer's diagnostic variables, read once (like each crate's
//! `*_debug_flags`): checks and debug views for development. The quality
//! settings are [`crate::settings::ModernSettings`].

use std::sync::OnceLock;

/// A debug view of the water pass (`CLIENT910_MODERN_WATER`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterDebug {
    /// `env`: no planar reflection (the environment sky only).
    EnvOnly,
    /// `debug<N>`: one term (1 fresnel over a flat normal, 2 the flow
    /// displacement, 3 the reflected colour, 4 the water body, 5 the
    /// reflection's world positions, 6 the shading depth, 7 the tangent
    /// normal, 8 the normal maps' mip levels).
    Term(u32),
}

/// A debug view of the terrain (`CLIENT910_MODERN_TERRAIN`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerrainDebug {
    /// `no-spec`: without the specular term.
    NoSpecular,
    /// `level<N>`: only level `N`'s terrain.
    Level(usize),
}

/// See the module docs.
#[derive(Debug, Default)]
pub struct ModernDebugFlags {
    /// `CLIENT910_MODERN_CHECK`: each frame, the shell compares the draw
    /// list with the faithful backend's mesh lists
    /// (`SceneMeshes::frame_summary`), and the subsystems check their
    /// inputs, and log the result.
    pub check: bool,
    /// `CLIENT910_MODERN_TEXTURES=bc|etc|png`: the RT7 texture source to use
    /// when the device can sample it; unset, the device's best
    /// (`crate::models::materials::TextureSource::for_device`).
    pub textures: Option<String>,
    /// `CLIENT910_MODERN_GRADING=off`: the ungraded tonemap.
    pub grading_off: bool,
    /// `CLIENT910_MODERN_PROBES=sh|ibl|metal`: the probe ambient's light, the
    /// image-based specular or the metalness instead of the lit colour (1, 2,
    /// 3).
    pub probe_debug: Option<u32>,
    pub water: Option<WaterDebug>,
    pub terrain: Option<TerrainDebug>,
    /// `CLIENT910_MODERN_THREADS=N`: the threads the frame's jobs use, the
    /// render thread included (`frame::jobs`); `1` encodes and poses on the
    /// render thread alone (tests, determinism debugging). Unset: the cores
    /// the far scene's streaming workers leave, 2 to
    /// `frame::jobs::DEFAULT_MAX_THREADS`.
    pub threads: Option<usize>,
}

/// The flags, parsed on first use.
pub fn flags() -> &'static ModernDebugFlags {
    static FLAGS: OnceLock<ModernDebugFlags> = OnceLock::new();
    FLAGS.get_or_init(|| {
        let var = |name: &str| std::env::var(name).ok();
        ModernDebugFlags {
            check: std::env::var_os("CLIENT910_MODERN_CHECK").is_some(),
            textures: var("CLIENT910_MODERN_TEXTURES"),
            grading_off: var("CLIENT910_MODERN_GRADING").is_some_and(|v| v == "off"),
            probe_debug: match var("CLIENT910_MODERN_PROBES").as_deref() {
                Some("sh") => Some(1),
                Some("ibl") => Some(2),
                Some("metal") => Some(3),
                _ => None,
            },
            water: match var("CLIENT910_MODERN_WATER").as_deref() {
                Some("env") => Some(WaterDebug::EnvOnly),
                Some(v) if v.starts_with("debug") => v[5..].parse().ok().map(WaterDebug::Term),
                _ => None,
            },
            terrain: match var("CLIENT910_MODERN_TERRAIN").as_deref() {
                Some("no-spec") => Some(TerrainDebug::NoSpecular),
                Some(v) if v.starts_with("level") => v[5..].parse().ok().map(TerrainDebug::Level),
                _ => None,
            },
            threads: var("CLIENT910_MODERN_THREADS")
                .and_then(|v| v.parse::<usize>().ok())
                .filter(|&n| n >= 1),
        }
    })
}
