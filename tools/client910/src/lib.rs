//! The 910 client. Client status and remaining work are in
//! `docs/client-status.md`. The scene viewer runs
//! offline; normal online play uses the authoritative runtime and player bodies.

mod active_toolkit;
mod app;
#[cfg(test)]
mod audio_corpus;
#[cfg(test)]
mod config_goldens;
mod debug_flags;
#[cfg(all(unix, any(test, feature = "dev")))]
mod live_control;
mod logging;
mod macos_defaults;
mod modern_display;
#[cfg(test)]
mod phase_g_checks;
#[cfg(test)]
mod server_rebuild_oracle;
#[cfg(test)]
mod server_teleport_oracle;
mod shutdown_signal;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod ui_camera_controls_tests;
mod ui_cursor_winit;
mod ui_keyboard_winit;
mod ui_mouse;
mod ui_window_winit;

use clap::Parser;

/// Client entry (called by `src/main.rs`): parse CLI, hand off to [`app::run`].
pub fn main() -> anyhow::Result<()> {
    // Diagnostic `CLIENT910_*` variables are parsed once, before anything
    // reads them (debug_flags.rs), then the leveled stderr logger is set.
    debug_flags::flags();
    rs910_scene::scene_debug_flags::flags();
    rs910_game::game_debug_flags::flags();
    rs910_ui::ui_debug_flags::flags();
    rs910_toolkit::toolkit_debug_flags::flags();
    rs910_render_gpu::render_debug_flags::flags();
    rs910_client::client_debug_flags::flags();
    logging::init();
    // SIGTERM/SIGINT/SIGHUP end the client through its normal exit path
    // (GPU idle, ordered teardown), not mid-frame (shutdown_signal.rs).
    shutdown_signal::install();
    // AppKit AutoFill off before any window exists (macos_defaults.rs).
    macos_defaults::register();
    // The engine profiler's dump (`CLIENT910_PROFILE_OUT`, a
    // `--features profile` build; lane E-A4).
    rs910_core::profile::set_gpu_timing(!debug_flags::flags().profile_gpu_off);
    if let Some(path) = &debug_flags::flags().profile_out {
        if !rs910_core::profile::COMPILED {
            log::warn!("[client910] CLIENT910_PROFILE_OUT needs a `--features profile` build");
        } else if let Err(error) = rs910_core::profile::start_dump(path) {
            log::warn!("[client910] profile dump {}: {error}", path.display());
        }
    }
    let cli = app::Cli::parse();
    app::run(cli)
}
