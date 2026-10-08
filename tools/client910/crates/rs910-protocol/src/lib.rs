//! `rs910-protocol`: the 910 wire protocol (`docs/architecture.md`). The
//! network protocol tables (server, client and login packets), the
//! byte half of the game-stream read loop, and the client message builders.
//!
//! Rules (tools/README.md "Crate conventions"):
//! - Depends on rs910-core only (plus anyhow/thiserror/log): no tokio, no
//!   sockets, no native910. Parsing and encoding are pure over byte slices;
//!   client910's `net.rs`/`session.rs` own the connections and re-export
//!   [`framing`] and [`server_prot`].
//! - Modules keep their client910 names so the `use rs910_protocol::{...}`
//!   facades in `client910/src/lib.rs` keep `crate::m::...` paths compiling.

pub mod client_command;
pub mod framing;
pub mod isaac_cipher;
pub mod payload_reader;
pub mod proto;
pub mod reflection_check;
pub mod rsa_block;
pub mod server_prot;
pub mod tiny_cipher;
pub mod ui_dialogue;
pub mod wire_cipher;
