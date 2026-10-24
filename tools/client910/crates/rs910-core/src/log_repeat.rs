//! Rate-limited warnings for per-frame/per-cycle errors, shared by client910
//! (`logging` re-exports the macro) and the split crates (Phase 2.5). std only:
//! the macro names `::log` at its call site, so only callers depend on `log`.

/// Every `REPEAT_PERIOD`-th repeat of a [`warn_repeated!`] site is printed
/// with its running count.
pub const REPEAT_PERIOD: u64 = 500;

/// Whether occurrence `n` (1-based) of a repeating message should print.
pub fn should_log_repeat(n: u64) -> bool {
    n == 1 || n.is_multiple_of(REPEAT_PERIOD)
}

/// Log a per-frame/per-cycle error at `warn`: the first occurrence verbatim,
/// then every [`REPEAT_PERIOD`]-th with `(repeated N times)`, keyed by call
/// site. Named through its module (`rs910_core::log_repeat::warn_repeated!`,
/// the re-export below), so callers depend on this module, not the crate root.
#[doc(hidden)]
#[macro_export]
macro_rules! __warn_repeated {
    ($($arg:tt)+) => {{
        static COUNT: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);
        let n = COUNT.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed) + 1;
        if $crate::log_repeat::should_log_repeat(n) {
            if n == 1 {
                ::log::warn!($($arg)+);
            } else {
                ::log::warn!("{} (repeated {n} times)", format_args!($($arg)+));
            }
        }
    }};
}
pub use crate::__warn_repeated as warn_repeated;
