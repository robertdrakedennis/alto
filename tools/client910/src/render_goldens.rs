//! Renderer-neutral frame gates in client910's lib (the replay gate runs
//! it): the offline scene's fixed-clock determinism gate over the scene
//! state every backend draws from, and the developer console's call
//! sequence. Both were hosted on the software toolkit's pixels until it was
//! removed (lane DROP-SW); pixels are not a contract, the state and the op
//! stream a toolkit receives are.

mod console;
mod fixed_clock;

/// FNV-1a over a value's `Debug` rendering, streamed (no string).
fn debug_digest(value: &impl std::fmt::Debug) -> u64 {
    struct Fnv(u64);
    impl std::fmt::Write for Fnv {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            for &b in s.as_bytes() {
                self.0 ^= u64::from(b);
                self.0 = self.0.wrapping_mul(0x100000001b3);
            }
            Ok(())
        }
    }
    let mut h = Fnv(0xcbf29ce484222325);
    std::fmt::write(&mut h, format_args!("{value:?}")).unwrap();
    h.0
}
