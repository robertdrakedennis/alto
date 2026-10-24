//! Protocol opcode/size tables for the 910 login + game stream (rev 910).
//!
//! Pure constants, no I/O. The tables (`login`, `server`, `client`: one
//! `pub const NAME: u8` per packet, `COUNT`, `size()` and `name()`) are
//! in `proto/tables.rs`, the same tables as `revisions/910/protocol/*.tsv`,
//! which the development server reads. `proto::tests` checks them against the
//! recording of the original client's tables.
//!
//! Framing rule (both directions, crypto disabled so opcodes are plain):
//! - size >= 0: fixed payload length, no length prefix.
//! - size == -1: one `u8` length prefix follows the opcode.
//! - size == -2: one big-endian `u16` length prefix follows the opcode.
//!   The read loop reads the (optionally ISAAC-masked) opcode, then a length
//!   byte for `size == -1` or a big-endian `u16` for `size == -2`.
//!
//! Opcodes the client never handles or sends have no meaningful name:
//! server 51 and 170 are `UNHANDLED_*` (the client logs out on receipt) and
//! client 15, 19, 38, 43 and 46 are `UNUSED_*`.

/// Lobby port. World `id` listens on `43594 + id`, so world 1 is 43595.
/// See `server/src/lostcity/engine/Lobby.ts` (`listen(43594)`)
/// and `server/src/lostcity/engine/World.ts` (`listen(43594 + this.id)`).
pub const LOBBY_PORT: u16 = 43594;

/// World login port for a 1-based world id.
#[must_use]
pub const fn world_port(world_id: u16) -> u16 {
    LOBBY_PORT + world_id
}

mod tables;
#[cfg(test)]
mod tests;
pub use tables::{client, login, server};
