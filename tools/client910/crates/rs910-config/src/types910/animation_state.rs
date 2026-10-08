//! The selection views of a sequence/effect (`sequence_types::Sequence::
//! selection`, `effect_types::Effect::selection`); `entities910::animation_state`
//! re-exports them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sequence {
    pub id: i32,
    pub frames: Option<usize>,
    pub skeletal: bool,
    pub restart: i32,
    pub priority: i32,
    pub stationary: i32,
    pub moving: i32,
}
#[derive(Clone, Debug)]
pub struct Effect {
    pub sequence: i32,
    pub looping: bool,
}
