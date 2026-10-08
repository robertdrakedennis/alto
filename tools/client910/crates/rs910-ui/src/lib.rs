//! `rs910-ui`: the retained UI of the client: the CS2 script host over
//! native910's VM, components and interfaces, the right-click menu, the
//! loading screen, the developer console, the client watch overlay, the
//! world map, preferences, social lists and the UI's audio routing, with the
//! per-frame interface output a toolkit executes (`ui_backend::Output`,
//! `rs910_toolkit::ui_output`).
//!
//! No wgpu and no winit: the native halves live outside (`*_gpu` in the
//! device layer, rs910-gpu-device: `client_watch_gpu`,
//! `ui_preferences_metric_gpu`; `*_winit` in the shell, client910:
//! `ui_cursor_winit`, `ui_keyboard_winit`, `ui_window_winit`, and
//! `ui_mouse`). The seams are plain data or a trait the native side
//! implements: `ui_window::NativeWindow`, `ui_preferences::MetricContext`
//! (the benchmark function), `rs910_toolkit::console_draw::ConsoleView`.
//!
//! - [`ui_runtime`]: `Runtime`/`Engine`, the CS2 host and packet router
//!   (with `ui_host`, `ui_hook_host`, `ui_hooks`, `ui_vars`, ...).
//! - Components and layout: [`ui_components`], [`ui_properties`],
//!   [`ui_layout`], [`ui_lifecycle`], [`ui_interaction`], [`ui_loop`].
//! - Drawing: [`ui_backend`] (the retained traversal recording a frame),
//!   [`ui_draw`], [`ui_leaf`], [`ui_fonts`], [`ui_sprites`], [`ui_icons`],
//!   [`ui_models`] (interface models; the draw itself is
//!   `rs910_scene::interface_model`), [`ui_menu_render`].
//! - [`client_game`]: `ClientGame`, rs910-game's `Game` plus the UI
//!   variable state; [`loading`], [`console`], [`message_box`],
//!   [`world_map`], [`world_map_client`], [`client_watch`], [`iface`].
//! - [`audio_runtime`], [`positioned_sound`]: the audio owner and the
//!   positioned loc/NPC/player sounds, over rs910-audio (they live here
//!   because they read the protocol, game and camera layers).
//! - [`ui_debug_flags`]: the diagnostic variables only the UI reads.
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::ui_runtime::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod audio_runtime;
pub mod client_game;
pub mod client_watch;
pub mod clipboard;
pub mod console;
pub mod debug_overlay;
pub mod host_name;
pub mod iface;
pub mod loading;
pub mod message_box;
pub mod ping;
pub mod positioned_sound;
pub mod ui_backend;
pub mod ui_bas;
pub mod ui_cache;
pub mod ui_cam2;
pub mod ui_chat;
pub mod ui_components;
pub mod ui_cursor;
pub mod ui_debug_flags;
pub mod ui_draw;
pub mod ui_fonts;
pub mod ui_hook_host;
pub mod ui_hooks;
pub mod ui_host;
pub mod ui_icons;
pub mod ui_interaction;
pub mod ui_inv;
pub mod ui_keyboard;
pub mod ui_layout;
mod ui_layout_constraints;
pub mod ui_leaf;
pub mod ui_lifecycle;
pub mod ui_loop;
pub mod ui_menu_render;
pub mod ui_minimenu;
pub mod ui_model_animation;
pub mod ui_model_transform;
pub mod ui_models;
pub mod ui_obj_queries;
pub mod ui_player_options;
pub mod ui_player_state;
pub mod ui_preferences;
pub mod ui_properties;
pub mod ui_resources;
pub mod ui_runtime;
mod ui_runtime_children;
pub mod ui_scene_options;
pub mod ui_scripts;
pub mod ui_social;
pub mod ui_sprites;
pub mod ui_stats;
pub mod ui_time;
pub mod ui_vars;
pub mod ui_window;
pub mod ui_world_list;
pub mod world_map;
pub mod world_map_client;

// `warn_repeated!` as `crate::logging::warn_repeated!`, as in client910.
use rs910_core::log_repeat as logging;

// The moved code names these through `crate::` (like client910's facades).
use rs910_audio::{audio_api, audio_backend, audio_stream};
use rs910_config::{
    avatar, billboard, client_options, config, flo, font_metrics, scenery_varbits, sprite_data,
    sprite_sheet, texture, ui_bytes, ui_component_fields, ui_configs, ui_db, ui_defaults,
    ui_quests, utf16_text, wordpack,
};
use rs910_core::{
    actor_matrix, animation_matrix, applet_params, char_case, client_state, colour, logic_clock,
    trig, ui_text_compare,
};
#[cfg(test)]
use rs910_game::camera_follow;
use rs910_game::{
    animation_playback, cutscene, entities910, entity_runtime, game_debug_flags, game_runtime,
    login_state, protocol910, ui_changes, ui_var_store,
};
use rs910_js5::{cache, js5_fetch};
use rs910_model::{floor, font_layout, gpumodel, icon_raster, modelunlit, particle};
use rs910_protocol::{client_command, framing, proto, server_prot, ui_dialogue};
use rs910_scene::{
    animation_assets, cam2_scene, camera, entity_elements, env, interface_model, minimap,
    npc_type_model, player_model, player_picking, rebuild, scene_player_pick, ui_icon_model,
    world_map_polygon,
};
use rs910_toolkit::{
    compressed_texture_format, console_draw, game_canvas, toolkit_debug_flags, ui_output, ui_paint,
};

/// The shared test helpers (rs910-core / rs910-js5 `test-hooks`), under the
/// path the moved tests use (`crate::test_support::...`).
#[cfg(test)]
mod test_support {
    pub use rs910_js5::test_support::require_pack;
}
