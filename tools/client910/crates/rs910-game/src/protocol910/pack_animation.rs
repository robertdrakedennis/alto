//! Sequence and spot config inputs for animation node selection. Decode readiness
//! does not mean animation frames/keyframes or models are loaded or playing.
use crate::animation_sequences as sequence_pack;
use crate::protocol910::pack_types as types_pack;
use crate::{cache::Pack, entities910::animation_state::Config, protocol910::effect_types::Effect};
use anyhow::Result;
pub use sequence_pack::Sequences;
use std::collections::BTreeMap;
pub struct Inputs {
    pub sequences: Sequences,
    pub effects: BTreeMap<i32, Effect>,
}
pub fn load(pack: &Pack, seqs: &types_pack::Records) -> Result<Inputs> {
    let sequences = sequence_pack::decode(pack, seqs)?;
    let (n, raw) = types_pack::records(pack, "spot.config", 8)?;
    let mut effects = BTreeMap::new();
    for id in 0..n {
        effects.insert(
            id,
            match raw.get(&id) {
                Some(b) => {
                    Effect::decode(id, b).map_err(|e| anyhow::anyhow!("effect {id}: {e:?}"))?
                }
                None => Effect::default(),
            },
        );
    }
    Ok(Inputs { sequences, effects })
}
impl Inputs {
    /// Overlay slots have the loaded wear-position count.
    pub fn selection(&self, wear_positions: usize) -> Config {
        Config {
            slots: wear_positions,
            sequences: self
                .sequences
                .sequences
                .iter()
                .map(|(&id, s)| (id, s.selection()))
                .collect(),
            effects: self
                .effects
                .iter()
                .map(|(&id, e)| (id, e.selection()))
                .collect(),
        }
    }
}
