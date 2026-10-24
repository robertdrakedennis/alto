//! Pack fixtures shared by the client crates' unit tests (moved from
//! client910's `test_support`, Phase 2.4). Test-only: the module is
//! `#[cfg(any(test, feature = "test-hooks"))]`, and client910 and the
//! rs910-* crates enable `test-hooks` through `[dev-dependencies]` only.
//!
//! The fast suite requires the revision-910 cache at `server/data/pack`. A
//! test that reads it asserts the fixture is present, so a missing pack fails
//! the test with a message naming the file.

use std::path::PathBuf;

use crate::cache::Pack;

pub use rs910_core::test_support::{client_dir, client_fixtures, pack_root, repo_root};

/// The pack root, after asserting that `file` (for example
/// `client.vorbis.js5`) exists in it.
#[track_caller]
pub fn require_pack_file(file: &str) -> PathBuf {
    let root = pack_root();
    let path = root.join(file);
    assert!(
        path.is_file(),
        "{} is missing: this test needs the revision-910 cache in server/data/pack",
        path.display()
    );
    root
}

/// A [`Pack`] over `server/data/pack`, after asserting that `file` exists.
#[track_caller]
pub fn require_pack(file: &str) -> Pack {
    Pack::open(require_pack_file(file))
}
