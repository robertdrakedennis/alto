//! NPC customisation config (`config_types::Npc::customisation`);
//! `protocol910::npc_custom` re-exports it.
use std::collections::BTreeMap;
pub struct Spec {
    pub recolour: Option<usize>,
    pub retexture: Option<usize>,
}
pub type Config = BTreeMap<i32, Spec>;
