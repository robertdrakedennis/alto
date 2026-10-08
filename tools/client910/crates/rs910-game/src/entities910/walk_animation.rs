//! The walk/idle animation state: the node
//! an actor keeps between cycles. The selection logic (and
//! the idle-animation predicates) is `protocol910::walk_animation`
//! since Phase 2.2.
use super::animation_state::Node;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Walk {
    pub node: Node,
    pub idle: bool,
}
