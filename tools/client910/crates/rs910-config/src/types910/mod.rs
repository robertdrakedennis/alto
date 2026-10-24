//! The entity-config decoders and their pack adapters: BAS, hitmark/headbar,
//! obj/NPC, effect, idk, sequence, var/varbit/script types, the defaults
//! groups and the title enums, with the bit reader and decode [`Error`] the
//! entity appliers share.
//!
//! `rs910_game::protocol910` re-exports the decoder modules, so
//! `protocol910::bas_types::...` etc. keep resolving. The small modules named
//! after an entity module (`animation_state`, `appearance`, `combat`,
//! `customisation`, `movement`, `npc`, `npc_custom`, `zone`) hold the
//! config-side views the decoders build (`Bas::movement`,
//! `Sequence::selection`, `Npc::packet_type`, ...); the game module of the
//! same name glob re-exports them.
pub mod animation_state;
pub mod appearance;
pub mod bas_types;
pub mod combat;
pub mod combat_types;
pub mod config_types;
pub mod customisation;
pub mod defaults;
pub mod effect_types;
pub mod error;
pub mod idk_types;
pub mod movement;
pub mod npc;
pub mod npc_custom;
/// Pack adapter for the defaults archive.
pub mod pack_defaults;
pub mod pack_types;
pub mod packet;
pub mod script_types;
pub mod sequence_types;
pub mod titles;
pub mod varbits;
pub mod variable_types;
pub mod variables;
pub mod zone;

pub use error::Error;
pub use packet::Packet;
type Result<T> = std::result::Result<T, Error>;
