//! The NPC-packet view of an NPC type (`config_types::Npc::packet_type`);
//! `protocol910::npc` re-exports it.
#[derive(Clone, Debug, PartialEq)]
pub struct NpcType {
    pub size: i32,
    pub turnspeed: i32,
    pub vislevel: i32,
    pub name: String,
    /// Overhead-icon `(sprite group, icon id)` pairs.
    pub head_icons: Option<Vec<(i32, i16)>>,
    /// Sprite group of the cover marker.
    pub covermarker: i32,
    /// Bit 0 lets a map square's spawn list stand this NPC in the title world.
    pub walkflags: i32,
    /// Serial id of the facing the NPC has when it spawns (`None`: the cache
    /// holds an id outside the eight directions).
    pub respawndir: Option<i32>,
    pub has_sound_or_multinpc: bool,
}
