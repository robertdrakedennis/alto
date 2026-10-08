//! Diagnostic `CLIENT910_*` environment variables, read once.
//!
//! Every production `CLIENT910_*` variable is parsed here into typed fields
//! the first time [`flags`] is called (`main` forces that at startup). Call
//! sites read `crate::debug_flags::flags().field` instead of calling
//! `std::env::var*` on the frame, logic-cycle or CS2-command path. Names and
//! semantics are unchanged: each field documents the expression the call
//! site used before (presence via `var_os(..).is_some()`, `var(..).ok()` for
//! UTF-8 values, and the same parse/trim/filter rules). Test and oracle-only
//! variables (`*_REPLAY`, `*_OUT`, `ALTO_*`) stay at their test sites.
//!
//! The deterministic input injectors (`CLIENT910_UI_CLICK`, `_KEY_INPUT`,
//! ...) and `CLIENT910_RECORD` are the client layer's
//! (`rs910_client::client_debug_flags`, Phase 4), as are the session
//! core's `_OUT_TRACE` and persisted-file paths (Phase 5).
//!
//! The variables a split crate reads live in that crate with the same parse
//! (forced by `main` too): `rs910_scene::scene_debug_flags`,
//! `rs910_game::game_debug_flags`, `rs910_ui::ui_debug_flags` (the UI's
//! traces), `rs910_toolkit::toolkit_debug_flags` (the three the UI and
//! the GPU renderer share; Phase 3.2), `rs910_render_gpu::render_debug_flags`
//! (the GPU renderer's traces and profiling switches) and
//! `rs910_client::client_debug_flags` (the session recorder
//! and the input injectors; Phase 4). What is left here is the shell's.

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::OnceLock;

/// Parsed diagnostic environment. See the module docs.
#[derive(Debug, Default)]
pub struct DebugFlags {
    // --- presence flags: `std::env::var_os(NAME).is_some()` ---
    pub cursor_trace: bool,
    pub light_trace: bool,
    pub particle_trace: bool,
    /// `CLIENT910_POSTFX_OFF`: force the post-effect capture off (A/B only).
    pub postfx_off: bool,
    /// `CLIENT910_TITLE_STATS`: the window title shows the camera tile, the
    /// frame rate and the player name instead of the game's title.
    pub title_stats: bool,

    // --- values ---
    pub cache_dir: Option<PathBuf>,
    /// `CLIENT910_JS5_TRACE`: when set (UTF-8), trace every
    /// `value.parse::<u32>().unwrap_or(50).max(1)` frames.
    pub js5_trace_every: Option<u32>,
    /// `CLIENT910_SCREENSHOT_SERIES=c1,c2,..` (trimmed, unparsable dropped).
    pub screenshot_series: Option<Vec<i32>>,
    /// `CLIENT910_SCREENSHOT_CYCLE` parsed as `i32`.
    pub screenshot_cycle: Option<i32>,
    /// `CLIENT910_LOSE_DEVICE=<logic cycle>`: the GPU device is destroyed
    /// once at that cycle, as a driver reset would, to watch the recovery.
    pub lose_device_at: Option<i32>,
    /// `CLIENT910_LOADING_SCREENSHOTS=n1,n2,..` (trimmed, unparsable dropped).
    pub loading_screenshots: Option<Vec<u32>>,
    /// `CLIENT910_WINDOW_SIZE=w,h`: only when exactly two positive values
    /// parse (untrimmed, unparsable dropped).
    pub window_size: Option<[u32; 2]>,
    /// `CLIENT910_LOG=off|error|warn|info|debug|trace` (see `logging`).
    pub log: Option<String>,
    /// `CLIENT910_PROFILE_OUT=<file>`: start the engine profiler
    /// (`rs910_core::profile`, a `--features profile` build) and dump every
    /// frame to the file as CSV (`tools/perf/profsum.py`).
    pub profile_out: Option<PathBuf>,
    /// `CLIENT910_PROFILE_GPU=0`: the profiler records without the GPU
    /// pass timestamps (`rs910_core::profile::set_gpu_timing`).
    pub profile_gpu_off: bool,
    /// `CLIENT910_SKY_DECOR=kind,texture,x,y,z,size[,colour];...`: every sky
    /// box gets these decors (fixed directions, the first lighting the
    /// others), the diagnostic that shows the sky decor path, which no 910
    /// content uses. A spec of other than six or seven integers is ignored.
    pub sky_decor: Vec<Vec<i32>>,
}

static FLAGS: OnceLock<DebugFlags> = OnceLock::new();

/// The process-wide flags, parsed from the environment on first use.
pub fn flags() -> &'static DebugFlags {
    FLAGS.get_or_init(DebugFlags::from_env)
}

impl DebugFlags {
    pub fn from_env() -> Self {
        Self::from_lookup(&|name| std::env::var_os(name))
    }

    /// Parse from any variable source (`std::env::var_os` in production).
    pub fn from_lookup(var_os: &dyn Fn(&str) -> Option<OsString>) -> Self {
        let has = |name: &str| var_os(name).is_some();
        // `std::env::var(name).ok()`: absent or non-UTF-8 is `None`.
        let var = |name: &str| var_os(name).and_then(|v| v.into_string().ok());
        let path = |name: &str| var_os(name).map(PathBuf::from);
        Self {
            cursor_trace: has("CLIENT910_CURSOR_TRACE"),
            light_trace: has("CLIENT910_LIGHT_TRACE"),
            particle_trace: has("CLIENT910_PARTICLE_TRACE"),
            postfx_off: has("CLIENT910_POSTFX_OFF"),
            title_stats: has("CLIENT910_TITLE_STATS"),
            cache_dir: path("CLIENT910_CACHE_DIR"),
            js5_trace_every: var("CLIENT910_JS5_TRACE")
                .map(|spec| spec.parse::<u32>().unwrap_or(50).max(1)),
            screenshot_series: var("CLIENT910_SCREENSHOT_SERIES")
                .map(|v| v.split(',').filter_map(|c| c.trim().parse().ok()).collect()),
            screenshot_cycle: var("CLIENT910_SCREENSHOT_CYCLE").and_then(|v| v.parse::<i32>().ok()),
            lose_device_at: var("CLIENT910_LOSE_DEVICE").and_then(|v| v.trim().parse::<i32>().ok()),
            loading_screenshots: var("CLIENT910_LOADING_SCREENSHOTS").map(|spec| {
                spec.split(',')
                    .filter_map(|v| v.trim().parse::<u32>().ok())
                    .collect()
            }),
            window_size: var("CLIENT910_WINDOW_SIZE").and_then(|size| {
                let dims: Vec<u32> = size.split(',').filter_map(|v| v.parse().ok()).collect();
                (dims.len() == 2 && dims.iter().all(|&v| v > 0)).then(|| [dims[0], dims[1]])
            }),
            log: var("CLIENT910_LOG"),
            profile_out: path("CLIENT910_PROFILE_OUT"),
            profile_gpu_off: var("CLIENT910_PROFILE_GPU").as_deref() == Some("0"),
            sky_decor: var("CLIENT910_SKY_DECOR")
                .map(|specs| {
                    specs
                        .split(';')
                        .map(|spec| {
                            spec.split(',')
                                .filter_map(|v| v.trim().parse().ok())
                                .collect::<Vec<i32>>()
                        })
                        .filter(|values| (6..=7).contains(&values.len()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup<'a>(vars: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn parses_values_like_the_former_call_sites() {
        let f = DebugFlags::from_lookup(&lookup(&[
            ("CLIENT910_JS5_TRACE", "x"),
            ("CLIENT910_SCREENSHOT_SERIES", " 10, a,20 "),
            ("CLIENT910_SCREENSHOT_CYCLE", " 5"),
            ("CLIENT910_WINDOW_SIZE", "800,600"),
            ("CLIENT910_SKY_DECOR", "0, 7, 0,-256,-256, 96; 1,2"),
        ]));
        assert_eq!(f.sky_decor, vec![vec![0, 7, 0, -256, -256, 96]]);
        assert!(!f.light_trace);
        assert_eq!(f.js5_trace_every, Some(50));
        assert_eq!(f.screenshot_series, Some(vec![10, 20]));
        // `v.parse::<i32>()` was untrimmed.
        assert_eq!(f.screenshot_cycle, None);
        assert_eq!(f.lose_device_at, None);
        assert_eq!(f.window_size, Some([800, 600]));
        let none = DebugFlags::from_lookup(&lookup(&[("CLIENT910_WINDOW_SIZE", "800,0")]));
        assert_eq!(none.window_size, None);
        assert_eq!(none.js5_trace_every, None);
    }
}
