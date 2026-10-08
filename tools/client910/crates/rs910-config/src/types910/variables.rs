//! Sparse variable-container values of one entity (`Player::variables`): the
//! value half of `protocol910::variables` (moved in Phase 2.2 so entity
//! state does not name the applier; `protocol910::variables` re-exports it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Value {
    Int(i32),
    Long(i64),
    String(String),
    FineCoord([i32; 4]),
}
