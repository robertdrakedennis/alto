//! The client core (`docs/architecture.md`): the session owners
//! and their logic, independent of the window, the toolkit and the event
//! loop. The shell (`client910::app`: winit, `ActiveToolkit`, the scene's
//! GPU caches) drives it.
//!
//! - [`phases`]: [`ClientCore`], the core's state and its logic phases;
//! - [`redraw`]: the frame redraw at the fixed cadence (the state-writing
//!   steps once per logic frame, presents in between) over the shell's
//!   [`RedrawShell`];
//! - [`environment`]: `EnvironmentManager`'s CPU half (map, override,
//!   fade), updated at P9 and R3;
//! - [`session_core`]: the `Session` state (game, retained UI, login
//!   machine, connections), the live read and packet drain, the logic
//!   update (game update and interface update) and the retained-UI input
//!   halves;
//! - [`session_io`]: the session's server connections;
//! - [`io`]: the [`Io`] boundary (clock, sockets, platform) and its live,
//!   recording and replay implementations;
//! - [`effects`]: the typed cross-owner requests (`ClientEffect`) the core
//!   hands the shell;
//! - [`persistence`]: the client files (`random.dat`, preferences, client
//!   varcs);
//! - [`session_slot`]: borrow helpers over the shell's session slot.

pub mod effects;
pub mod environment;
pub mod input_event;
pub mod io;
pub mod persistence;
pub mod phases;
pub mod redraw;
pub mod session_core;
pub mod session_io;
pub mod session_slot;

pub use effects::*;
pub use environment::*;
pub use input_event::*;
pub use io::*;
pub use persistence::*;
pub use phases::*;
pub use redraw::*;
pub use session_core::*;
pub use session_io::*;
pub use session_slot::*;

// The names the moved items used from the shell's `app` module (they were
// `use super::*` there); `use super::*` in the submodules keeps them bare.
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use std::thread;

use anyhow::Context;

use crate::cache::Pack;
use crate::login_worker::ReconnectResponse;
use crate::net::LoginProfile;
use crate::toolkit_caps::ToolkitCaps;
