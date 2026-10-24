//! `rs910-client`: the client layer of the 910 client port (session IO and
//! the login connection side;
//! `docs/architecture.md`). It may name every crate below the shell except the two render
//! impls; tokio is fenced to it and rs910-js5.
//!
//! - [`net`]: sockets and the lobby/world/account-creation login handshakes
//!   (tokio) over rs910-protocol's framing and native910's `ByteWriter`.
//! - [`session`]: the world login and startup drain, and the live frame IO,
//!   over rs910-protocol's `server_prot` parsers.
//! - [`login_worker`]: the login, reconnect and account-creation worker
//!   threads and the runtime they (and the `--direct-login` startup) run on.
//! - [`loading_connection`]: the loading screen's keepalive connection.
//! - [`connection_upkeep`]: the world's silence timer, the ping report and
//!   the lobby keepalive's clock.
//! - [`login_crypto`]: the login RSA key configuration, session seeds and the
//!   sealing of a login block.
//! - [`wire_stream`]: a connection's socket with the game stream's opcode
//!   masking applied transparently.
//! - [`session_record`]: `CLIENT910_RECORD`, the RTR1 "lite" recorder.
//! - [`input_script`]: the deterministic input injectors;
//!   [`client_debug_flags`] parses them and the recorder's file once.
//! - [`graphics_runtime`]: the main redraw's precise-sleep tail.
//! - [`toolkit_caps`]: the renderer capabilities the session's toolkit
//!   installation reads.
//! - [`client_core`]: the client core (Phase 5): `ClientCore` (the
//!   session owners and the logic phase list), the `Io` boundary with its
//!   live, recording and replay implementations, the live read, the logic
//!   update, the core's redraw steps and the client files.
//!
//! The app (`client910::app`: `ViewerApp`, the winit `ApplicationHandler`,
//! `ActiveToolkit`, the scene's GPU owners) is the shell that drives
//! `ClientCore` (Phase 5). Modules keep their client910 names, so the facade in
//! `client910/src/lib.rs` keeps `crate::session::...` etc. compiling
//! (tools/README.md "Crate conventions").

pub mod client_core;
pub mod client_debug_flags;
pub mod connection_upkeep;
pub mod graphics_runtime;
pub mod input_script;
pub mod loading_connection;
pub mod login_crypto;
pub mod login_worker;
pub mod net;
pub mod session;
pub mod session_record;
pub mod toolkit_caps;
pub mod wire_stream;

// The moved code names these through `crate::` (like client910's facades).
use rs910_config::{avatar, config, ui_defaults};
use rs910_core::applet_params;
use rs910_core::log_repeat as logging;
use rs910_core::logic_clock;
use rs910_game::{entities910, entity_runtime, login_state, protocol910, ui_var_store};
use rs910_js5::cache;
use rs910_protocol::{client_command, proto, reflection_check};
use rs910_scene::map;
use rs910_ui::{client_game, client_watch, debug_overlay, ui_chat, ui_debug_flags, ui_interaction};
use rs910_ui::{positioned_sound, ui_runtime, ui_window};
