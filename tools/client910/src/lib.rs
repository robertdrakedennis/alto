//! The 910 client. Client status and remaining work are in
//! `docs/client-status.md`. The scene viewer runs
//! offline; normal online play uses the authoritative runtime and player bodies.

mod active_toolkit;
#[cfg(test)]
mod actor_oracle;
#[cfg(test)]
mod actor_render_oracle;
#[cfg(test)]
mod animation_oracle;
mod app;
#[cfg(test)]
mod audio_corpus;
#[cfg(test)]
mod audio_goldens;
#[cfg(test)]
mod camera_follow_oracle;
#[cfg(test)]
mod client_command_oracle;
#[cfg(test)]
mod config_goldens;
#[cfg(test)]
mod console_oracle;
#[cfg(test)]
mod core_goldens;
mod debug_flags;
#[cfg(test)]
mod dynamic_oracle;
#[cfg(test)]
mod entity_apply_bench;
#[cfg(test)]
mod equipment_observer_replay;
#[cfg(test)]
mod font_atlas_oracle;
#[cfg(test)]
mod font_layout_oracle;
#[cfg(test)]
mod font_metrics_oracle;
#[cfg(test)]
mod game_goldens;
#[cfg(unix)]
mod live_control;
mod logging;
mod macos_defaults;
#[cfg(test)]
mod model_goldens;
mod modern_display;
#[cfg(test)]
mod phase_g_checks;
#[cfg(test)]
mod player_animation_replay;
#[cfg(test)]
mod player_model_oracle;
#[cfg(test)]
mod player_pose_oracle;
#[cfg(test)]
mod player_scene_oracle;
#[cfg(test)]
mod protocol_goldens;
#[cfg(test)]
mod recorded_golden;
#[cfg(test)]
mod render_goldens;
#[cfg(test)]
mod scene_golden;
#[cfg(test)]
mod scene_goldens;
#[cfg(test)]
mod server_rebuild_oracle;
#[cfg(test)]
mod server_teleport_oracle;
mod shutdown_signal;
#[cfg(test)]
mod sprite_draw_oracle;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod text_render_oracle;
#[cfg(test)]
mod ui_camera_controls_tests;
#[cfg(test)]
mod ui_client_state_oracle;
#[cfg(test)]
mod ui_component_fields_snapshot;
#[cfg(test)]
mod ui_components_oracle;
#[cfg(test)]
mod ui_configs_oracle;
mod ui_cursor_winit;
#[cfg(test)]
mod ui_draw_oracle;
#[cfg(test)]
mod ui_equipment_replay;
#[cfg(test)]
mod ui_fonts_oracle;
#[cfg(test)]
mod ui_goldens;
#[cfg(test)]
mod ui_hooks_oracle;
mod ui_keyboard_winit;
#[cfg(test)]
mod ui_leaf_oracle;
#[cfg(test)]
mod ui_lifecycle_oracle;
mod ui_mouse;
#[cfg(test)]
mod ui_obj_oracle;
#[cfg(test)]
mod ui_opkey_trace;
#[cfg(test)]
mod ui_properties_oracle;
#[cfg(test)]
mod ui_resources_oracle;
#[cfg(test)]
mod ui_scripts_oracle;
#[cfg(test)]
mod ui_sprites_oracle;
#[cfg(test)]
mod ui_vars_oracle;
mod ui_window_winit;

// Phase 2.1 facades: these modules moved to rs910-core
// (tools/client910/crates/rs910-core); `crate::m::...` paths keep compiling.
// A later codemod (programme R1/R2) rewrites the paths and removes them.
#[cfg(test)]
use rs910_core::ui_text_compare;
use rs910_core::{actor_matrix, applet_params, logic_clock, trig};
#[cfg(test)]
use rs910_core::{animation_matrix, colour, png_out};

// Phase 2.3 facades: the cache and JS5 network modules moved to rs910-js5
// (tools/client910/crates/rs910-js5); `crate::cache::...` and
// `crate::js5net::...` paths keep compiling.
use rs910_js5::{cache, js5net};
// Phase 2.2 facades: these modules moved to rs910-protocol
// (tools/client910/crates/rs910-protocol); `crate::m::...` paths keep compiling.
#[cfg(test)]
use rs910_protocol::reflection_check;
#[cfg(test)]
use rs910_protocol::ui_dialogue;
use rs910_protocol::{client_command, proto};
// Phase 2.5 facades: these modules moved to rs910-audio
// (tools/client910/crates/rs910-audio).
#[cfg(test)]
use rs910_audio::{audio_api, audio_backend, audio_stream};

// Phase 2.4 facades: the config-type modules moved to rs910-config
// (tools/client910/crates/rs910-config); `crate::config::...` etc. keep
// compiling.
#[cfg(test)]
use rs910_config::anim;
#[cfg(test)]
use rs910_config::animation_curve;
#[cfg(test)]
use rs910_config::ui_component_fields;
#[cfg(test)]
use rs910_config::ui_defaults;
#[cfg(test)]
use rs910_config::ui_legacy_types;
use rs910_config::{billboard, config, flo, texture};
#[cfg(test)]
use rs910_config::{client_options, font_metrics, sprite_data};
// Phase 3.2: wordpack, ui_configs and what ui_configs needed from the UI.
#[cfg(test)]
use rs910_config::ui_configs;

// Phase 2.7 facades: the CPU model modules moved to rs910-model
// (tools/client910/crates/rs910-model); `crate::gpumodel::...` etc. keep
// compiling.
#[cfg(test)]
use rs910_model::animation_skeletal;
use rs910_model::{floor, gpumodel, hardshadow, modelunlit, particle};
#[cfg(test)]
use rs910_model::{font_layout, material, water};

// Phase 2.6 facades: entity state and the packet appliers moved to rs910-game
// (tools/client910/crates/rs910-game); `crate::protocol910::...` etc. keep
// compiling.
#[cfg(test)]
use rs910_game::actor;
#[cfg(test)]
use rs910_game::ui_changes;
use rs910_game::{
    animation_playback, camera_follow, entities910, entity_runtime, login_state, protocol910,
};
// Phase 3.2: the game layer's diagnostic (CLIENT910_CUTSCENE_TRACE).
use rs910_game::game_debug_flags;
// Phase 3.2 facades: `game_runtime` and `cutscene` moved to rs910-game,
// `game_scene` (the `GameScene` extension of `Game`) to rs910-scene.
#[cfg(test)]
use rs910_game::cutscene;
use rs910_scene::game_scene;
// Phase 3.2: floor_render's CPU uniform block, for ui_models, and the
// interface-model draw the UI hands the renderers.
use rs910_scene::interface_model;
// Renderer plan A1: the renderer-neutral scene snapshot.
use rs910_scene::scene_snapshot;

// Phase 2.8 splits (tools/refactor/steps/p2-scene-split.py): items that
// moved into lower crates, reached as `crate::m::...` by client910 code.

// Phase 2.8 facades: the CPU scene modules moved to rs910-scene
// (tools/client910/crates/rs910-scene); `crate::scene::...` etc. keep
// compiling. `draw_trace` stays public for the drawdiff bin.
pub use rs910_scene::draw_trace;
#[cfg(test)]
use rs910_scene::occlusion;
use rs910_scene::{
    animation_assets, camera, cover_marker, draw, dynamic_loc, dynamic_scene, entity_elements, env,
    floorlight, live_scene, locs, loctype, map, maploader, minimap, npc_draw, npc_type_model,
    obj_stack, occlusion_fixtures, player_body, player_picking, player_scene, rebuild,
    roof_fixtures, scene, skybox, title_world,
};
#[cfg(test)]
use rs910_scene::{player_model, player_pose, player_shadow, scene_player_pick, ui_icon_model};

// Phase 3.1 facades: the display lists, the `Toolkit` trait, the CPU font and
// the FramePlan moved to rs910-toolkit (tools/client910/crates/rs910-toolkit);
// `crate::ui_paint::...` etc. keep compiling. `fetch_file` moved to
// rs910-js5, `SpriteSheet` and `font_atlas` to rs910-config (the `iface`
// split).
#[cfg(test)]
use rs910_config::{font_atlas, sprite_sheet};
#[cfg(test)]
use rs910_js5::js5_fetch;
use rs910_toolkit::ui_paint;
#[cfg(test)]
use rs910_toolkit::{font, sprite_draw, text_render};
// Phase 3.2: the diagnostics the UI and the GPU renderer share.
use rs910_toolkit::toolkit_debug_flags;
// Phase 3.2: the UI's frame output and the canvas geometry.
use rs910_toolkit::ui_output;
// Phase 3.2: the console's draw plan.
use rs910_toolkit::console_draw;
// Phase 3.2: the CPU halves of the capability queries.

// Phase 3.2 facades: the retained UI moved to rs910-ui
// (tools/client910/crates/rs910-ui); `crate::ui_runtime::...` etc. keep
// compiling.
use rs910_ui::{
    audio_runtime, client_game, clipboard, console, debug_overlay, loading, message_box,
    positioned_sound, ui_backend, ui_cam2, ui_cursor, ui_fonts, ui_models, ui_preferences,
    ui_runtime, ui_scene_options, ui_sprites, ui_window,
};
#[cfg(test)]
use rs910_ui::{
    iface, ui_cache, ui_components, ui_draw, ui_hook_host, ui_hooks, ui_interaction, ui_leaf,
    ui_lifecycle, ui_loop, ui_minimenu, ui_player_options, ui_properties, ui_resources, ui_scripts,
    ui_social, ui_stats, ui_vars,
};

// Phase 3.3/3.4 facades: the renderers moved to rs910-render-gpu and
// rs910-gpu-device (tools/client910/crates); the shell
// keeps `active_toolkit` and the `*_winit` halves. `crate::render::...` etc.
// keep compiling.
use rs910_gpu_device::{gpu_device, ui_preferences_metric_gpu};
#[cfg(test)]
use rs910_render_gpu::{actor_render, sprite_draw_gpu, ui_model_gpu, ui_paint_gpu};
use rs910_render_gpu::{
    floor_render, particle_render, player_renderer, postprocess, render, render_debug_flags,
    scene_meshes, skybox_render,
};

// Phase 4 facades: the client layer moved to rs910-client
// (tools/client910/crates/rs910-client); client910 is the shell.
// `crate::session::...` etc. keep compiling.
#[cfg(test)]
use rs910_client::connection_upkeep;
#[cfg(test)]
use rs910_client::input_script;
use rs910_client::{
    client_core, client_debug_flags, graphics_runtime, loading_connection, login_crypto,
    login_worker, net, session, session_record, toolkit_caps, wire_stream,
};

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
