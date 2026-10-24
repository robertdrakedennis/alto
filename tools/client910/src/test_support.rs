//! Shared test helpers. The session-replay helpers live with the maintainer-only
//! sessions and are present only when `build.rs` enables them.
pub use rs910_js5::test_support::require_pack;

#[cfg(alto_sessions)]
#[path = "../fixtures/session-replay/rust/support.rs"]
mod support;
#[cfg(alto_sessions)]
pub use support::*;
