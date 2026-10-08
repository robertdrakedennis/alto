//! `rs910-core`: the std-only leaf utilities of the 910 client port
//! (`docs/architecture.md`). The core utilities, math and encryption helpers that
//! every other crate reads.
//!
//! Rules (tools/README.md "Crate conventions"):
//! - std only: no `[dependencies]` (checked by `tools/refactor/dag-check.py`).
//! - One copy per concept. Modules keep their client910 names so the
//!   `pub use` facades in `client910/src/lib.rs` keep `crate::m::...` paths
//!   compiling.
//! - Fallible helpers return [`Result`] with the std-only [`Error`], whose
//!   `Display` is the message the old `anyhow` error printed; callers that
//!   return `anyhow::Result` keep using `?`.

pub mod actor_matrix;
pub mod animation_matrix;
pub mod animation_random;
pub mod applet_params;
pub mod cache_schedule;
pub mod char_case;
pub mod checksum;
pub mod client_state;
pub mod colour;
pub mod cp1252;
pub mod fault;
pub mod hardware;
pub mod log_repeat;
pub mod logic_clock;
pub mod perlin;
pub mod png_out;
pub mod profile;
pub mod reader;
pub mod soft_cache;
#[cfg(any(test, feature = "test-hooks"))]
pub mod test_support;
pub mod texts;
mod texts_table;
pub mod trig;
pub mod ui_text_compare;
pub mod vector_math;
pub mod whirlpool;

use std::fmt;

/// Error of the fallible core helpers: a message, displayed verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error(String);

impl Error {
    /// An error displaying `message`.
    pub fn msg(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self(e.to_string())
    }
}

/// `Result` with the core [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// `anyhow::anyhow!` for [`Error`]: format a message.
#[macro_export]
macro_rules! err {
    ($($arg:tt)*) => {
        $crate::Error::msg(format!($($arg)*))
    };
}

/// `anyhow::ensure!` for [`Error`]: return the formatted error unless `cond`.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err($crate::err!($($arg)*));
        }
    };
}
