//! `rs910-game`: the game-state layer of the 910 client port (`docs/architecture.md`):
//! entity state, the player/NPC info and packet appliers, the var
//! domains and the session state machine: thin oracle #3
//! (movement/varp/varbit/varc state per tick) runs over this crate.
//!
//! - [`entities910`]: player/NPC state (movement, animation, appearance,
//!   combat, chat, varps); [`protocol910`]: PLAYER_INFO/NPC_INFO, zone,
//!   varp/varbit, live-feed and rebuild appliers over it (its config decoders
//!   live in `rs910_config::types910` and are re-exported there);
//!   [`entity_runtime`]: the pack-backed inputs and packet runtime.
//! - [`actor`] (player/NPC logic updates) and
//!   [`animation_playback`] (animation frame progression);
//!   [`camera_follow`].
//! - [`client_vars`], [`ui_var_store`], [`ui_changes`]: client variable
//!   values, their persistence and delayed state changes.
//! - [`login_state`]: the client state machine and rebuild states.
//! - [`game_runtime`]: `Game`, the logged-in state the packet read and the
//!   game update drive (packet apply, actor/projectile updates,
//!   var polling); [`cutscene`]: the cutscene manager and the action clock
//!   (Phase 3.2, from client910).
//! - [`game_debug_flags`]: the diagnostic variable only this layer reads.
//! - [`screen_fade`]: the screen fade state (Phase 3.2, from ui_draw).
//!
//! Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::protocol910::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod actor;
pub mod animation_playback;
pub mod camera_follow;
pub mod client_vars;
pub mod cutscene;
pub mod entities910;
#[path = "protocol910/pack_runtime.rs"]
pub mod entity_runtime;
pub mod game_debug_flags;
pub mod game_runtime;
pub mod login_state;
pub mod protocol910;
pub mod screen_fade;
pub mod sequence_sound;
pub mod ui_changes;
pub mod ui_var_store;

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{animation_sequences, config, ui_bytes, ui_defaults};
use rs910_core::{logic_clock, trig};
use rs910_js5::{cache, js5_fetch};
use rs910_protocol::proto;
