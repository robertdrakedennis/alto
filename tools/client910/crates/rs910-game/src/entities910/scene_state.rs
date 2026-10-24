//! Actor-owned scene state, retained across packet updates.
//! Plain CPU data so protocol/runtime consumers do not depend on rendering.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub priority: i32,
    pub deferred: bool,
    pub use_idle: bool,
    pub force_show: bool,
    pub bounds: [i32; 4],
    pub min_y: i32,
    pub height: i32,
    pub ground: [i32; 3],
    pub decoration_offset: i32,
    pub transparent: bool,
    pub draw_cycle: i32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            priority: 0,
            deferred: true,
            use_idle: false,
            force_show: false,
            bounds: [0; 4],
            min_y: -32768,
            height: -32768,
            ground: [0; 3],
            decoration_offset: 0,
            transparent: false,
            draw_cycle: 0,
        }
    }
}
