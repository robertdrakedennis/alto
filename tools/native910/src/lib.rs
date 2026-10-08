#![deny(clippy::dbg_macro, clippy::todo, clippy::unimplemented)]
// `unwrap_used` is enforced in production builds only; `.unwrap()` is idiomatic in unit tests.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]
// The curated policy allow-list lives in `Cargo.toml [lints.clippy]` so it applies to ALL targets
// (lib, bins, examples, tests) — a crate-level `#![allow]` here would not reach the separate
// example/test crates.

//! `native910` — the 910-first native cache toolkit for alto.
//!
//! Revision 910 (10 Dec 2019) is the only revision this crate knows. There are no donor builds,
//! no fallback opcode books, no cross-revision heuristics: bytes that are not 910-shaped are a
//! hard error. Later cross-revision work consumes this crate's output; it never lives inside it.
//!
//! Milestone order: CS2 scripts (decode/assemble byte-exact) → interfaces → configs → repack.
//! Every codec ships with a byte-round-trip gate: `decode(encode(x)) == x` over the real 910
//! corpus before any edit workflow builds on top of it.

pub mod assets;
pub mod cli;
pub mod config;
pub mod dbtable;
pub mod effects;
pub mod error;
pub mod evidence;
pub mod execution;
pub mod expr;
pub mod inames;
pub mod interface;
pub mod iparse;
pub mod isource;
pub mod js5;
pub mod jstr;
pub mod opcode;
pub mod pack;
pub mod packet;
pub mod parse;
pub mod returns;
pub mod script;
pub mod source;
pub mod sprite;
pub mod symbols;
pub mod validate;
pub mod vars;
pub mod vm;
pub mod xref;

pub mod semantics;

pub mod preview;

pub mod runtime;

pub mod repack;

pub mod project;

pub mod debug;

pub mod dataflow;

pub mod coverage;

pub mod resources;

/// Committed per-command stack contracts (test seam: integration tests diff
/// the public contracts against this table; not a stable API).
#[doc(hidden)]
pub mod cs2_stack_contracts;
