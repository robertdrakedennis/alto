//! Rebuild footprints of loc types, read from the decoded loc store.
use crate::config::LocStore;
use std::collections::BTreeMap;
pub struct Inputs {
    pub sizes: BTreeMap<i32, (i32, i32)>,
}
/// The footprint of every id below `count` (the loc archive's capacity);
/// an id with no file is one square.
pub fn footprints(locs: &LocStore, count: i32) -> Inputs {
    let mut sizes = BTreeMap::new();
    for id in 0..count {
        let size = match locs.get(id as u32) {
            Some(loc) => (loc.width as i32, loc.length as i32),
            None => (1, 1),
        };
        sizes.insert(id, size);
    }
    Inputs { sizes }
}
