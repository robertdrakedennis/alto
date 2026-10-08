//! Clean exit on SIGTERM, SIGINT and SIGHUP.
//!
//! Without a handler these signals end the process at once, mid-frame, with
//! GPU work in flight and the wgpu device, surface and Metal layer never torn
//! down. Scripts stop clients with `timeout`/`kill` (SIGTERM), and on macOS
//! many such abrupt exits were seen alongside growing wired memory. The
//! handler only records the request; the event loop sees it at the next
//! `about_to_wait` and leaves through winit's normal exit, which runs
//! `exiting` (the client shutdown, then [`ActiveToolkit::wait_idle`]) and drops
//! the GPU objects in order. A second signal while the first is pending
//! exits immediately (a client stuck before its next frame still stops).
//!
//! [`ActiveToolkit::wait_idle`]: crate::active_toolkit::ActiveToolkit::wait_idle
use std::sync::atomic::{AtomicBool, Ordering};

static REQUESTED: AtomicBool = AtomicBool::new(false);

#[cfg(unix)]
extern "C" fn on_signal(signal: libc::c_int) {
    // Async-signal-safe: an atomic swap, and `_exit` for the second signal.
    if REQUESTED.swap(true, Ordering::SeqCst) {
        // SAFETY: `_exit` is async-signal-safe and does not return.
        unsafe { libc::_exit(128 + signal) };
    }
}

/// Installs the handlers (once, at start-up).
pub fn install() {
    #[cfg(unix)]
    for signal in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        // SAFETY: `on_signal` only touches an atomic and calls `_exit`, both
        // async-signal-safe; the handler lives for the whole process.
        unsafe {
            libc::signal(
                signal,
                on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t,
            );
        }
    }
}

/// Whether a stop signal arrived (the event loop then exits cleanly).
pub fn requested() -> bool {
    REQUESTED.load(Ordering::SeqCst)
}
