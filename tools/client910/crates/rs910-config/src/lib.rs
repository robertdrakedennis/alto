//! `rs910-config`: the config-type layer of the client: decoders from cache
//! bytes to typed stores. Each decoder is driven by a static opcode table
//! ([`opcode_table`]) that holds the revision's opcode layout; the types
//! themselves are plain structs.
//!
//! - [`config`]: loc/npc/obj/seq/inv types and their stores; [`flo`] (floor
//!   overlays and underlays), [`texture`] (textures and materials), [`anim`]
//!   (keyframe sets and bases) with [`animation_curve`] (their curve
//!   evaluator), [`avatar`] (identity kits and the kit palette),
//!   [`billboard`].
//! - [`font_metrics`], [`sprite_data`]: font and sprite resources.
//! - [`ui_bytes`] (the UI decoders' `anyhow` reader), [`ui_component_fields`],
//!   [`ui_legacy_types`], [`ui_quests`], [`ui_defaults`] (menu defaults and
//!   the bindings decoder).
//! - [`types910`]: the entity-config decoders (`*_types`), their pack
//!   adapters and the bit reader the entity appliers share;
//!   [`animation_sequences`], [`scenery_varbits`] (pack loaders over them),
//!   [`ui_db`] (database rows and tables) and [`cutscene_file`] (the
//!   `cutscenes` file format).
//! - [`wordpack`]: chat text compression over the cache's huffman table;
//!   [`ui_configs`] (enum and struct lists, their script accessors, and
//!   `ParamConfig`) and [`utf16_text`] (the UI's UTF-16 string type).
//! - [`client_options`]: `ClientOptions`, its v38 codec and presets (thin
//!   oracle #2).
//! - [`nxt`]: side tables for the extra data of newer caches (RT7 material
//!   extras, map files 5-8, texture headers of archives 52-55) used by the
//!   opt-in NXT-style renderer.
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::config::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod anim;
pub mod animation_curve;
pub mod animation_sequences;
pub mod avatar;
pub mod billboard;
pub mod client_options;
pub mod config;
pub mod cutscene_file;
pub mod flo;
pub mod font_atlas;
pub mod font_metrics;
pub mod landscape_packet;
pub mod loc_sound;
pub mod login_configs;
pub mod npc_customisation;
pub mod nxt;
pub mod opcode_table;
pub mod scenery_varbits;
pub mod skybox_types;
pub mod sprite_data;
pub mod sprite_sheet;
pub mod texture;
pub mod types910;
pub mod ui_bytes;
pub mod ui_component_fields;
pub mod ui_configs;
pub mod ui_db;
mod ui_db_resource;
pub mod ui_db_schema;
pub mod ui_defaults;
pub mod ui_enum_resource;
pub mod ui_enum_schema;
pub mod ui_legacy_types;
pub mod ui_quests;
pub mod utf16_text;
pub mod wordpack;

// The moved code names these through `crate::` (like client910's facades).
use rs910_core::{colour, trig};
use rs910_js5::{cache, js5_fetch};

#[cfg(test)]
mod corpus;

/// The shared test helpers (rs910-js5 `test-hooks`), under the path the
/// moved tests use (`crate::test_support::...`).
#[cfg(test)]
mod test_support {
    pub use rs910_js5::test_support::require_pack;
}
