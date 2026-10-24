//! Hitmark/headbar config views (`combat_types::*::combat`);
//! `protocol910::combat` re-exports them.
pub struct HitType {
    pub replace: i32,
    pub duration: i32,
}
pub struct BarType {
    pub show: i32,
    pub hide: i32,
    pub duration: i32,
}
