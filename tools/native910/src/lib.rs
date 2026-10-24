#![deny(clippy::dbg_macro, clippy::todo, clippy::unimplemented)]
// `unwrap_used` is enforced in production builds only; `.unwrap()` is idiomatic in unit tests.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
// The curated policy allow-list lives in `Cargo.toml [lints.clippy]` so it applies to ALL targets
// (lib, bins, examples, tests) — a crate-level `#![allow]` here would not reach the separate
// example/test crates.

//! `native910`: the client's revision-910 cache codecs (JS5 containers, packs,
//! configs, interfaces, sprites, database tables) and the CS2 virtual machine with
//! its opcode book and command semantics.
//!
//! Revision 910 (10 Dec 2019) is the only revision this crate knows: bytes that
//! are not 910-shaped are a hard error.

pub mod config;
pub mod dbtable;
pub mod effects;
pub mod error;
pub mod interface;
pub mod js5;
pub mod jstr;
pub mod opcode;
pub mod pack;
pub mod packet;
pub mod script;
pub mod sprite;
pub mod vars;
pub mod vm;

pub mod semantics;

pub mod runtime;

/// Committed per-command stack contracts (test seam: integration tests diff
/// the public contracts against this table; not a stable API).
#[doc(hidden)]
pub mod cs2_stack_contracts;
