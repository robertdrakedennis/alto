//! The movement view of a BAS (`bas_types::Bas::movement`);
//! `entities910::movement` re-exports it.
#[derive(Clone, Debug)]
pub struct Bas {
    pub walk_speed: i32,
    pub turn_accel: i32,
    pub turn_max: i32,
    pub roll_accel: i32,
    pub roll_max: i32,
    pub roll_target: i32,
    pub pitch_accel: i32,
    pub pitch_max: i32,
    pub pitch_target: i32,
}
impl Default for Bas {
    fn default() -> Self {
        Self {
            walk_speed: -1,
            turn_accel: 0,
            turn_max: 0,
            roll_accel: 0,
            roll_max: 0,
            roll_target: 0,
            pitch_accel: 0,
            pitch_max: 0,
            pitch_target: 0,
        }
    }
}
