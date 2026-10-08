//! Crate error type. Library code returns `Result<T>`; only the CLI boundary
//! converts to `anyhow` for user-facing reports.

use thiserror::Error;

/// Every failure this crate can produce.
#[derive(Debug, Error)]
pub enum NativeError {
    /// A read ran past the end of the buffer.
    #[error("unexpected end of data reading {what}")]
    Truncated {
        /// What was being read (e.g. `"u16 opcode"`, `"NUL-terminated name"`).
        what: &'static str,
    },
    /// The bytes are well-formed enough to read but not valid 910 data.
    #[error("invalid data: {0}")]
    Invalid(String),
    /// Filesystem IO.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, NativeError>;
