//! Protocol opcode/size tables for the 910 login + game stream (rev 910).
//!
//! Pure constants, no I/O. The tables (`login`, `server`, `client`: one
//! `pub const NAME: u8` per packet, `COUNT`, `size()` and `name()`) are
//! generated into `proto/tables.rs` from `revisions/910/protocol/*.tsv` by
//! `tools/revision/gen_protocol.py`, the same tables the development server
//! is generated from. Change a packet's opcode, size or name in the table,
//! never in the generated file.
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
pub use tables::{client, login, server};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_table_is_complete() {
        // 12 login opcodes.
        assert_eq!(login::COUNT, 12);
        let cases: &[(u8, i32, &str)] = &[
            (14, 0, "INIT_GAME_CONNECTION"),
            (15, -1, "INIT_JS5REMOTE_CONNECTION"),
            (16, -2, "GAMELOGIN"),
            (19, -2, "LOBBYLOGIN"),
            (23, 4, "REQUEST_WORLDLIST"),
            (24, -1, "CHECK_WORLD_SUITABILITY"),
            (26, 0, "GAMELOGIN_CONTINUE"),
            (27, 0, "SSL_WEBCONNECTION"),
            (28, -2, "CREATE_ACCOUNT_CONNECT"),
            (29, -2, "INIT_SOCIAL_NETWORK_CONNECTION"),
            (30, -2, "SOCIAL_NETWORK_LOGIN"),
            (31, 4, "INIT_DEBUG_CONNECTION"),
        ];
        for (opcode, size, name) in cases {
            assert_eq!(login::size(*opcode), Some(*size), "login {opcode}");
            assert_eq!(login::name(*opcode), *name, "login {opcode}");
        }
        assert_eq!(login::size(0), None);
        assert_eq!(login::size(13), None);
        assert_eq!(login::name(99), "UNKNOWN");
    }
}
