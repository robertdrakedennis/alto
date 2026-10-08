//! The app session's transport state (code-quality programme Phase 4.4): the
//! game and lobby server connections with the bytes read
//! but not yet framed, the replies queued for the next non-blocking flush
//! and the framing resync cursor, plus the per-connection statistics
//! the debug overlay reads and the keep-alives of the loading screens.
//!
//! The socket loops themselves are `rs910_client::session::{flush_nonblocking,
//! read_nonblocking}` (the Phase 4.3 IO seam), reached through the core's
//! `Io` (Phase 5: live, recorded or replayed).

/// One server connection: the non-blocking socket, the
/// unframed input and the queued output.
#[derive(Default)]
pub struct Connection {
    /// The open socket; absent on the title screen, in the lobby (for the
    /// world connection) and after a logout closed it.
    pub stream: Option<crate::wire_stream::WireStream<std::net::TcpStream>>,
    /// Leftover frame bytes across polls (a `REBUILD_NORMAL` can arrive split
    /// across TCP segments).
    pub pending: Vec<u8>,
    /// Queued reply bytes (`MAP_BUILD_COMPLETE`/`NO_TIMEOUT`) flushed with
    /// `try_write` each frame (never blocks).
    pub pending_writes: Vec<u8>,
    /// Resync cursor for [`crate::net::decode_frame_game`] (unknown bytes
    /// skipped with a warn, never fatal).
    pub resync: crate::net::ResyncState,
}

/// The session's connections (world and lobby) and their keep-alive state.
pub struct SessionIo {
    /// The loading-screen keep-alive of the direct-login world connection,
    /// finished once the window exists (`ViewerApp::resumed`).
    pub startup_connection: Option<crate::loading_connection::LoadingConnection>,
    /// The idle write ticks of the world connection
    /// (`NO_TIMEOUT` after 50 idle logic cycles).
    pub idle_connection: crate::loading_connection::IdleConnection,
    /// The same keepalive of the lobby connection.
    pub lobby_idle_connection: crate::loading_connection::IdleConnection,
    /// Cycles without a byte from the world; too many reconnect.
    pub incoming_idle: crate::connection_upkeep::IncomingIdle,
    /// The ping report of the game state.
    pub ping: crate::connection_upkeep::PingReporter,
    /// The game (world) connection.
    pub world: Connection,
    /// The lobby connection, used while the retained login tree is
    /// active.
    pub lobby: Connection,
    /// `gameConnection` / `lobbyConnection` byte counters for `drawDebug`.
    pub net_stats: [crate::debug_overlay::NetStats; 2],
}
