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

#[cfg(test)]
mod tests {
    #[test]
    fn cpu_sleeps_table_follows_the_cpu_usage_setting() {
        // cpuUsage 0/1/2/3 map to sleeps of 15/10/5/2 ms and anything else to
        // no sleep; the multiple of 10 splits into 9 + 1 ms, hence [9, 1].
        // State-only: table values, never wall-clock timing.
        assert_eq!(super::cpu_sleeps(0), &[15]);
        assert_eq!(super::cpu_sleeps(1), &[9, 1]);
        assert_eq!(super::cpu_sleeps(2), &[5]);
        assert_eq!(super::cpu_sleeps(3), &[2]);
        assert_eq!(super::cpu_sleeps(4), &[] as &[u64]);
        assert!(super::cpu_sleeps(-1).is_empty());
        assert!(super::cpu_sleeps(5).is_empty());
        assert!(super::cpu_sleeps(99).is_empty());
    }
}
