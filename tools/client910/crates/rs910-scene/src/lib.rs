//! `rs910-scene`: the CPU scene layer of the client (`docs/architecture.md`).
//! The world (scene graph, map loader, occlusion), the scene entities, the
//! camera and the minimap: everything between the decoded config/model data
//! and a toolkit. No wgpu: the GPU passes
//! (`floor_render`, `floorpass`, `billboard_render`, `skybox_render`, ...)
//! are rs910-render-gpu's.
//!
//! - Map build: [`map`] (region groups, LAND/LOC decode), [`map_npcs`] (the
//!   NPC spawn lists the title world stands up), [`maploader`]
//!   (the map loader: landscape, floors), [`tileflags`], [`locs`] (loc
//!   placement), [`rebuild`] (the scene build), [`env`] (environment,
//!   lights), [`floorlight`], [`model_lights`], [`light_animation`],
//!   [`scene`] (the scene graph).
//! - Draw: [`draw`] (the scene draw planner), [`draw_entity`],
//!   [`draw_trace`] (DRW1), [`occlusion`], [`occlusion_raster`], [`roof`];
//!   [`occlusion_fixtures`]/[`roof_fixtures`] feed the replay oracles.
//! - Live state: [`live_scene`], [`dynamic_loc`], [`dynamic_scene`],
//!   [`obj_stack`], [`skybox`] (the sky box state; its config types are
//!   `rs910_config::skybox_types`), [`sky_frame`] (one frame's skybox as the
//!   renderers draw it, Phase 3.3, and `SkyCache`, its CPU resolution,
//!   lane Q-PREM1), [`sky_decor`] (the decor sprites' bake), [`sky_texture`]
//!   (the sky's cache stores and texture load), [`title_world`].
//! - Camera and picking: [`camera`], [`cam2_scene`], [`player_picking`],
//!   [`scene_player_pick`].
//! - Minimap and overlays: [`minimap`], [`world_map_polygon`],
//!   [`cover_marker`], [`entity_elements`].
//! - Entity models: [`loctype`], [`npc_type_model`], [`player_model`],
//!   [`player_pose`], [`player_body`], [`player_shadow`], [`player_scene`],
//!   [`animation_assets`], [`ui_icon_model`]; [`player_draw`] (what a
//!   toolkit's player draw reads of the player owner; Phase 3.3).
//! - [`scene_debug_flags`]: the diagnostic variables only this layer reads.
//! - [`interface_model`]: one interface model draw as the UI hands it to
//!   the renderers (`ui_models::Draw`; Phase 3.2).
//! - [`scene_snapshot`]: `SceneSnapshot`, the renderer-neutral scene of one
//!   frame every backend draws from (NXT renderer plan A1), and the backend
//!   contract (its module docs).
//! - [`floor_uniforms`]: the model/floor shaders' CPU-built uniform block
//!   (Phase 3.2, from floor_render).
//! - [`game_scene`]: `GameScene`, the scene-graph reads of rs910-game's `Game`
//!   (renderer/CPU map agreement, loc snapshots; Phase 3.2).
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::scene::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod animation_assets;
pub mod cam2_scene;
pub mod camera;
#[cfg(test)]
mod corpus;
pub mod cover_marker;
pub mod draw;
pub mod draw_entity;
pub mod draw_trace;
pub mod dynamic_loc;
pub mod dynamic_scene;
pub mod entity_elements;
pub mod env;
pub mod floor_uniforms;
pub mod floorlight;
pub mod game_scene;
pub mod interface_model;
pub mod light_animation;
pub mod live_scene;
pub mod locs;
pub mod loctype;
pub mod map;
pub mod map_npcs;
pub mod maploader;
pub mod minimap;
pub mod model_lights;
pub mod npc_draw;
pub mod npc_type_model;
pub mod obj_stack;
pub mod occlusion;
pub mod occlusion_fixtures;
pub mod occlusion_raster;
pub mod player_body;
pub mod player_draw;
pub mod player_model;
pub mod player_picking;
pub mod player_pose;
pub mod player_scene;
pub mod player_shadow;
pub mod rebuild;
pub mod roof;
pub mod roof_fixtures;
pub mod scene;
pub mod scene_debug_flags;
pub mod scene_player_pick;
pub mod scene_snapshot;
pub mod sky_decor;
pub mod sky_frame;
pub mod sky_texture;
pub mod skybox;
pub mod tileflags;
pub mod title_world;
pub mod ui_icon_model;
pub mod world_map_polygon;

// The moved code reads its diagnostics as `crate::debug_flags::flags()`, as
// in client910; the module has its own name so the DAG scan does not merge it
// with client910's `debug_flags`.
use scene_debug_flags as debug_flags;

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{
    anim, animation_sequences, avatar, billboard, client_options, config, flo, landscape_packet,
    loc_sound, npc_customisation, scenery_varbits, sprite_data, texture, ui_bytes,
};
use rs910_core::{actor_matrix, colour, logic_clock, trig, vector_math};
use rs910_game::{animation_playback, entities910, entity_runtime, game_runtime, protocol910};
use rs910_js5::cache;
use rs910_model::{
    animation_skeletal, floor, font_layout, gpumodel, hardshadow, icon_raster, material, model,
    modelunlit, particle, sprite,
};

/// The shared test helpers (rs910-core / rs910-js5 `test-hooks`), under the
/// path the moved tests use (`crate::test_support::...`).
#[cfg(test)]
mod test_support {
    pub use rs910_js5::test_support::require_pack;
}
