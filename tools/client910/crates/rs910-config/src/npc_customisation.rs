//! The per-component NPC model/recolour overrides the NPC head/body model
//! builders consume. Split out of client910's `ui_models` (Phase 2.8) so
//! `npc_type_model` leaves the UI; `ui_models` re-exports it.

use anyhow::Context;
use rs910_core::fault::Fault;

/// Per-component NPC model overrides, carried by an
/// interface component (`Component.customisation`) and consumed by the NPC
/// head/body model builders above.
#[derive(Clone, Debug, PartialEq)]
pub struct NpcCustomisation {
    pub cache_key_salt: i64,
    pub models: Vec<i32>,
    pub custom_scale: Vec<f32>,
    pub custom_rotation: Vec<[i32; 3]>,
    pub custom_offset: Vec<[i32; 3]>,
    pub custom_recol_d: Option<Vec<i16>>,
    pub custom_retex_d: Option<Vec<i16>>,
}

impl NpcCustomisation {
    /// The customisation of an NPC's body models (`body`) or head models.
    pub fn new(npc: &crate::config::Npc, body: bool) -> anyhow::Result<Self> {
        let models = if body {
            npc.model_slots.clone()
        } else {
            npc.head_slots
                .clone()
                .with_context(|| Fault::MissingValue.message("NPC head slots"))?
        };
        let n = models.len();
        Ok(Self {
            cache_key_salt: 0,
            models,
            custom_scale: vec![0.0; n],
            custom_rotation: vec![[0; 3]; n],
            custom_offset: vec![[0; 3]; n],
            custom_recol_d: (!npc.recol_s.is_empty() || !npc.recol_d.is_empty())
                .then(|| npc.recol_d.iter().map(|&v| v as i16).collect()),
            custom_retex_d: (!npc.retex_s.is_empty() || !npc.retex_d.is_empty())
                .then(|| npc.retex_d.iter().map(|&v| v as i16).collect()),
        })
    }
    /// Grow the per-slot tables so `slot` is valid.
    fn ensure_capacity(&mut self, slot: usize) {
        if slot < self.models.len() {
            return;
        }
        self.models.resize(slot + 1, -1);
        self.custom_scale.resize(slot + 1, 0.0);
        self.custom_rotation.resize(slot + 1, [0; 3]);
        self.custom_offset.resize(slot + 1, [0; 3]);
    }
    /// Set the model, scale, rotation and offset of one slot.
    pub fn set_model_transform(
        &mut self,
        slot: usize,
        model: i32,
        scale: f32,
        rotation: [i32; 3],
        offset: [i32; 3],
    ) {
        self.ensure_capacity(slot);
        self.models[slot] = model;
        self.custom_scale[slot] = scale;
        self.custom_rotation[slot] = rotation;
        self.custom_offset[slot] = offset;
    }
    /// Replace the model of one slot.
    pub fn set_model(&mut self, slot: usize, model: i32) {
        self.ensure_capacity(slot);
        self.models[slot] = model;
    }
}
