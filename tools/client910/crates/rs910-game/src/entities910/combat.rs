//! Hitmark and headbar queues. Order is the queue's front/previous order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Combat {
    pub cursor: usize,
    pub hits: Vec<[i32; 5]>,
    pub bars: Vec<Bar>,
}
impl Combat {
    pub fn new(slots: usize) -> Self {
        Self {
            cursor: 0,
            hits: vec![[0; 5]; slots],
            bars: vec![],
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bar {
    pub id: i32,
    pub updates: Vec<[i32; 4]>,
}
