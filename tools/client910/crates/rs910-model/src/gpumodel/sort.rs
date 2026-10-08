//! The face sort: a quicksort of 64-bit keys that carries a parallel array of
//! values along, with a deterministic jitter on the pivot comparison. The
//! exact order of equal and near-equal keys decides the draw order of faces,
//! so the algorithm is kept as it is.

/// Sorts `keys` ascending and moves `values` with them.
pub fn quicksort_parallel(keys: &mut [i64], values: &mut [i32]) {
    if keys.is_empty() {
        return;
    }
    sort_range(keys, values, 0, keys.len() as i32 - 1);
}

fn sort_range(keys: &mut [i64], values: &mut [i32], low: i32, high: i32) {
    if low >= high {
        return;
    }
    let pivot_at = ((low + high) / 2) as usize;
    let mut store = low as usize;
    let end = high as usize;
    let pivot_key = keys[pivot_at];
    keys.swap(pivot_at, end);
    let pivot_value = values[pivot_at];
    values.swap(pivot_at, end);
    // The jitter: every other index compares against the pivot plus one,
    // except for the maximum key, which never moves.
    let jitter_mask: i64 = if pivot_key == i64::MAX { 0 } else { 1 };
    for i in low as usize..end {
        if keys[i] < (i as i64 & jitter_mask) + pivot_key {
            keys.swap(i, store);
            values.swap(i, store);
            store += 1;
        }
    }
    keys[end] = keys[store];
    keys[store] = pivot_key;
    values[end] = values[store];
    values[store] = pivot_value;
    sort_range(keys, values, low, store as i32 - 1);
    sort_range(keys, values, store as i32 + 1, high);
}
