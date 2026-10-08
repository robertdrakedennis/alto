//! The login, reconnect and account-creation workers (the login connection
//! IO, run off the winit thread so the client loop never blocks)
//! and the tokio runtime they run on. Split out of client910's app.rs in
//! Phase 4 (lane Q-CLIENT) as whole items, so the shell names no tokio path
//! (tokio is fenced to rs910-js5 and rs910-client, target-architecture.md
//! §2.2). Each worker thread builds a current-thread runtime, runs one
//! login future (cancellable through its `AtomicBool`) and sends one
//! [`ReconnectResponse`] to the app's channel, which the app drains in its
//! logic cycle; the sockets it hands back leave the worker's reactor through
//! [`detach_world_socket`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use anyhow::Context;

/// The socket a worker hands back (`LobbyReady`, `CreateReady`), still
/// registered with the worker's reactor until [`detach_world_socket`] moves
/// it out.
pub type WorkerStream = crate::wire_stream::WireStream<tokio::net::TcpStream>;

/// A current-thread runtime with every driver enabled: what each worker
/// builds, and the `--direct-login` startup (`online_login`) blocks on.
pub fn current_thread_runtime() -> std::io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

pub enum ReconnectResponse {
    Ready {
        generation: u64,
        /// Boxed: the drained world state dwarfs the other variants.
        live: Box<crate::session::LiveState>,
    },
    LobbyReady {
        generation: u64,
        stream: WorkerStream,
        /// Boxed: the profile and the resume block outsize the other variants.
        login: Box<crate::net::LoginOk>,
    },
    CreateReady {
        generation: u64,
        stream: WorkerStream,
        reply: i32,
    },
    CreateFailed {
        generation: u64,
        error: String,
    },
    Failed {
        generation: u64,
        error: String,
        transfer: Option<crate::net::LoginTransferFailure>,
        reply_state: Option<crate::net::LoginReplyState>,
        /// Reply -5 or -4 once the attempts are exhausted.
        exhausted: Option<i32>,
        /// The world login failed after the server had accepted it into the
        /// lobby's world (at `GAMELOGIN_CONTINUE`): a lobby login then falls
        /// back to the lobby by logging out.
        late: bool,
    },
}

impl ReconnectResponse {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Ready { generation, .. }
            | Self::LobbyReady { generation, .. }
            | Self::CreateReady { generation, .. }
            | Self::CreateFailed { generation, .. }
            | Self::Failed { generation, .. } => *generation,
        }
    }
}

async fn wait_for_reconnect_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Run the transfer login off the winit thread. The login state is entered
/// without blocking the client loop; this worker performs the same world
/// handshake and initial frame drain while the retained login tree stays live.
pub fn spawn_reconnect_worker(
    generation: u64,
    cancel: Arc<AtomicBool>,
    params: crate::net::LoginParams,
    strict_entities: bool,
    response_tx: Sender<ReconnectResponse>,
) {
    let spawned = thread::Builder::new()
        .name("client910-world-reconnect".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = response_tx.send(ReconnectResponse::Failed { generation, error: format!("runtime: {error:#}"), transfer: None, reply_state: None, exhausted: None, late: false });
                    return;
                }
            };
            let result = runtime.block_on(async {
                tokio::select! {
                    result = crate::session::login_world_and_drain(&params, strict_entities) => Some(result),
                    () = wait_for_reconnect_cancel(cancel) => None,
                }
            });
            if let Some(result) = result {
                let response = match result {
                    Ok(live) => ReconnectResponse::Ready { generation, live: Box::new(live) },
                    Err(error) => {
                        let transfer = error.downcast_ref::<crate::net::LoginTransferFailure>().cloned();
                        let reply_state = error.downcast_ref::<crate::net::LoginReplyState>().cloned();
                        let exhausted = error.downcast_ref::<crate::net::LoginAttemptsExhausted>().map(|e| e.reply);
                        let late = error.downcast_ref::<crate::net::LateLoginFailure>().is_some();
                        ReconnectResponse::Failed { generation, error: format!("{error:#}"), transfer, reply_state, exhausted, late }
                    },
                };
                let _ = response_tx.send(response);
            }
        });
    if let Err(error) = spawned {
        log::warn!("[client910] world reconnect worker spawn failed: {error}");
    }
}

pub fn spawn_lobby_login_worker(
    generation: u64,
    cancel: Arc<AtomicBool>,
    params: crate::net::LoginParams,
    response_tx: Sender<ReconnectResponse>,
) {
    let spawned = thread::Builder::new()
        .name("client910-lobby-login".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = response_tx.send(ReconnectResponse::Failed {
                        generation,
                        error: format!("lobby runtime: {error:#}"),
                        transfer: None,
                        reply_state: None,
                        exhausted: None,
                        late: false,
                    });
                    return;
                }
            };
            let result = runtime.block_on(async {
                tokio::select! {
                    result = crate::net::login_lobby(&params) => Some(result),
                    () = wait_for_reconnect_cancel(cancel) => None,
                }
            });
            if let Some(result) = result {
                let response = match result {
                    Ok((stream, login)) => ReconnectResponse::LobbyReady {
                        generation,
                        stream,
                        login: Box::new(login),
                    },
                    Err(error) => {
                        let reply_state =
                            error.downcast_ref::<crate::net::LoginReplyState>().cloned();
                        let exhausted = error
                            .downcast_ref::<crate::net::LoginAttemptsExhausted>()
                            .map(|e| e.reply);
                        ReconnectResponse::Failed {
                            generation,
                            error: format!("lobby login: {error:#}"),
                            transfer: None,
                            reply_state,
                            exhausted,
                            late: false,
                        }
                    }
                };
                let _ = response_tx.send(response);
            }
        });
    if let Err(error) = spawned {
        log::warn!("[client910] lobby login worker spawn failed: {error}");
    }
}

/// Run the account-creation connection handshake away from the winit
/// thread. The resulting socket is installed as the lobby client-protocol
/// owner only after the one-byte connect reply arrives.
pub fn spawn_create_connect_worker(
    generation: u64,
    host: String,
    port: u16,
    info: crate::net::CreateConnectInfo,
    response_tx: Sender<ReconnectResponse>,
) {
    let spawned = thread::Builder::new()
        .name("client910-create-connect".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = response_tx.send(ReconnectResponse::CreateFailed {
                        generation,
                        error: format!("create runtime: {error:#}"),
                    });
                    return;
                }
            };
            let result =
                runtime.block_on(crate::net::create_account_connect_at(&host, port, &info));
            let response = match result {
                Ok((stream, reply)) => ReconnectResponse::CreateReady {
                    generation,
                    stream,
                    reply,
                },
                Err(error) => ReconnectResponse::CreateFailed {
                    generation,
                    error: format!("create connect: {error:#}"),
                },
            };
            let _ = response_tx.send(response);
        });
    if let Err(error) = spawned {
        log::warn!("[client910] account-creation worker spawn failed: {error}");
    }
}

pub fn detach_world_socket(
    stream: WorkerStream,
) -> anyhow::Result<crate::wire_stream::WireStream<std::net::TcpStream>> {
    let stream = stream
        .into_std()
        .context("transfer world socket out of startup reactor")?;
    stream.set_nonblocking(true)?;
    Ok(stream)
}
