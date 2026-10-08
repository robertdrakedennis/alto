//! Tests of rs910-ui's `ui_runtime` that need client910: the recorded-session
//! replays over `fixtures/replays` (the replay gate's retained-UI entries),
//! `session::handle_sync_frame`, `app`, `player_renderer`, `ui_model_gpu`
//! and `graphics_runtime`. Moved with unchanged bodies from `ui_runtime`'s
//! test modules (Phase 3.2); each globs `ui_runtime` like the `use super::*`
//! it came from (tools/README.md "Tests").

/// The CS2 minimenu queries read the engine's menu snapshot (pack-free). The
/// server menu-state packets are driven end to end by
/// `app::scenario_tests::live_server_packets_reach_their_consumers`.
mod menu_state_tests;
mod player_picking_tests;
mod settings_matrix;
mod settings_profiling;
pub(crate) mod settings_world;
mod social_packet_tests;
mod stat_packet_tests;
#[cfg(unix)]
mod ui_authoring_replay;
mod ui_consumable_replay;
#[cfg(unix)]
mod ui_imported_replay;
mod ui_run_replay;
mod ui_settings_replay;
