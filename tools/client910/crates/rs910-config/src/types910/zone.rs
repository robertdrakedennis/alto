//! The ground-object view of an obj type (`config_types::Item::ground_type`);
//! `protocol910::zone` re-exports it.
pub struct ObjectType {
    pub cost: i32,
    pub stackable: i32,
}
