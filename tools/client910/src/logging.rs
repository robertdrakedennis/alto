//! Minimal leveled stderr logger behind the `log` facade.
//!
//! Only records whose target is this crate (`client910::*`) or one of the
//! crates split out of it (`rs910_*::*`, Phase 2) are printed, so
//! dependency logging (wgpu, naga, ...) stays as silent as before. Records
//! print their message text only (no level/target prefix), so the lines
//! other tooling greps for (`[client910] ui diagnostics: ...`,
//! `screenshot written`, client state transitions, `login failed`) keep
//! their exact text.
//!
//! Level: `CLIENT910_LOG=off|error|warn|info|debug|trace` (default `info`).
//! Per-packet/per-frame success chatter is `debug`; one-off state changes,
//! diagnostics and errors are `info`/`warn`. Errors that can repeat every
//! frame or cycle go through [`warn_repeated!`], which prints the first
//! occurrence and then a periodic count.

use log::{LevelFilter, Log, Metadata, Record};

struct StderrLogger;

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        let target = metadata.target();
        target.starts_with("client910") || target.starts_with("rs910_")
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            eprintln!("{}", record.args());
        }
    }

    fn flush(&self) {}
}

static LOGGER: StderrLogger = StderrLogger;

/// `CLIENT910_LOG` as a level filter (unknown values keep the default).
pub fn level_filter(spec: Option<&str>) -> LevelFilter {
    match spec.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("off" | "0" | "none") => LevelFilter::Off,
        Some("error") => LevelFilter::Error,
        Some("warn" | "warning") => LevelFilter::Warn,
        Some("debug") => LevelFilter::Debug,
        Some("trace" | "all") => LevelFilter::Trace,
        _ => LevelFilter::Info,
    }
}

/// Install the logger once at startup (`main`).
pub fn init() {
    let level = level_filter(crate::debug_flags::flags().log.as_deref());
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(level);
    }
}

// `warn_repeated!` and its period live in rs910-core (Phase 2.5) so the
// split crates share them; `crate::logging::warn_repeated!` still works.
pub(crate) use rs910_core::log_repeat::warn_repeated;

#[cfg(test)]
mod tests {
    use super::*;

    fn enabled(target: &str) -> bool {
        StderrLogger.enabled(&Metadata::builder().target(target).build())
    }

    /// Records from client910 and the crates split out of it (Phase 2 moves
    /// `js5net`'s `log::info!`s to target `rs910_js5::js5net`) print;
    /// dependency logging stays silent.
    #[test]
    fn prints_client_and_split_crate_targets_only() {
        assert!(enabled("client910::app"));
        assert!(enabled("rs910_js5::js5net"));
        assert!(enabled("rs910_core"));
        assert!(!enabled("wgpu_core::device"));
        assert!(!enabled("naga::front"));
    }
}
