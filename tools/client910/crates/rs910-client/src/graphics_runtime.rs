//! The end-of-frame CPU-usage sleep. One sleep is requested per frame from the
//! `cpuUsage` preference: `0 → 15`, `1 → 10`, `2 → 5`, `3 → 2`, anything else
//! → none. A precise sleep splits exact multiples of 10 into `n - 1` plus 1
//! milliseconds, so the table of individual sleeps here is `[15]`, `[9, 1]`,
//! `[5]`, `[2]`, `[]`.
//! `finish_frame` performs the sleeps; `cpu_sleeps` exposes the pure table for
//! state-only tests (no wall-clock assertions).
pub fn cpu_sleeps(value: i32) -> &'static [u64] {
    match value {
        0 => &[15],
        1 => &[9, 1],
        2 => &[5],
        3 => &[2],
        _ => &[],
    }
}
pub fn finish_frame(cpu_usage: i32) {
    for &millis in cpu_sleeps(cpu_usage) {
        std::thread::sleep(std::time::Duration::from_millis(millis));
    }
}
