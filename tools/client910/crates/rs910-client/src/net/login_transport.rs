//! Async login orchestration, held replies, retries and profile decoding.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};

use native910::packet::Packet;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use tokio::net::TcpStream;

use tokio::time::{sleep, timeout};

use crate::wire_stream::WireStream;

use rs910_protocol::wire_cipher::{SessionSeeds, WireCipher};

use super::{
    classify_login_code, game_login_continue_packet, hop_retry_after_ms,
    init_game_connection_packet, retry_after, retry_read, seal_create_account_connect_packet,
    seal_game_login_packet, seal_lobby_login_packet, seal_social_connection_packet,
    seal_social_login_packet, stage_timeout_duration, Attempt, ConnectionKind, CreateConnectInfo,
    LateLoginFailure, LobbyProfile, LoginAttemptsExhausted, LoginClock, LoginCode, LoginOk,
    LoginParams, LoginProfile, LoginProgress, LoginReplyState, LoginTransferFailure,
    LoginWaitExpired, LOGIN_MAX_ATTEMPTS,
};

/// The connect reply that accepts an account-creation connection.
pub(super) const CONNECT_REPLY_SUCCESSFUL: u8 = 2;

/// Open the account-creation lobby transport and return the live socket
/// after the one-byte connect reply decision. The returned stream is already
/// in the normal lobby client-protocol phase on success (masked with the
/// handshake block's seeds when the connect is encrypted; the three request
/// kinds of account creation then also travel under the tiny cipher).
pub async fn create_account_connect_at(
    host: &str,
    port: u16,
    info: &CreateConnectInfo,
) -> Result<(WireStream<TcpStream>, i32)> {
    let (packet, seeds) = seal_create_account_connect_packet(info)?;
    let addr = format!("{host}:{port}");
    let mut stream = timeout(
        stage_timeout_duration(true, false),
        TcpStream::connect(&addr),
    )
    .await
    .map_err(|_| anyhow!("account creation connect {addr} timed out"))??;
    stream
        .write_all(&packet)
        .await
        .context("send CREATE_ACCOUNT_CONNECT")?;
    let mut reply = [0u8; 1];
    timeout(
        stage_timeout_duration(true, false),
        stream.read_exact(&mut reply),
    )
    .await
    .map_err(|_| anyhow!("account creation connect reply timed out"))??;
    let cipher = seeds
        .filter(|_| reply[0] == CONNECT_REPLY_SUCCESSFUL)
        .map(WireCipher::for_account_creation);
    Ok((WireStream::new(stream, cipher), i32::from(reply[0])))
}

// ---------------------------------------------------------------------------
// Async socket layer.
// ---------------------------------------------------------------------------

pub(super) async fn read_exact_clock<S>(
    stream: &mut S,
    buf: &mut [u8],
    clock: &LoginClock,
    what: &str,
) -> Result<()>
where
    S: AsyncRead + Unpin,
{
    match clock.remaining() {
        Some(remaining) if remaining.is_zero() => {
            return Err(LoginWaitExpired(what.to_owned()).into());
        }
        Some(remaining) => {
            timeout(remaining, stream.read_exact(buf))
                .await
                .map_err(|_| LoginWaitExpired(what.to_owned()))?
                .with_context(|| what.to_owned())?;
        }
        None => {
            stream
                .read_exact(buf)
                .await
                .with_context(|| what.to_owned())?;
        }
    }
    Ok(())
}

pub(super) async fn read_byte_clock<S>(stream: &mut S, clock: &LoginClock, what: &str) -> Result<u8>
where
    S: AsyncRead + Unpin,
{
    let mut byte = [0u8; 1];
    read_exact_clock(stream, &mut byte, clock, what).await?;
    Ok(byte[0])
}

pub(super) async fn read_u16_clock<S>(stream: &mut S, clock: &LoginClock, what: &str) -> Result<u16>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = [0u8; 2];
    read_exact_clock(stream, &mut bytes, clock, what).await?;
    Ok(u16::from_be_bytes(bytes))
}

pub(super) async fn read_u32_clock<S>(stream: &mut S, clock: &LoginClock, what: &str) -> Result<u32>
where
    S: AsyncRead + Unpin,
{
    let mut bytes = [0u8; 4];
    read_exact_clock(stream, &mut bytes, clock, what).await?;
    Ok(u32::from_be_bytes(bytes))
}

/// Outcome of [`read_handshake_token`]: a fatal server reject carries the
/// code; anything else is a retryable transport/timeout failure.
pub(super) enum Handshake {
    Token(i64),
    Rejected(u8),
}

/// Read the 9-byte `INIT_GAME_CONNECTION` reply: `p1(code) + p8(token)`.
/// Code `0` yields [`Handshake::Token`]; anything else yields
/// [`Handshake::Rejected`] (login ends immediately, as at login step 35).
/// Transport/timeout failures are `Err` (retryable by the caller).
pub(super) async fn read_handshake_token<S>(
    stream: &mut S,
    where_: &str,
    clock: &LoginClock,
    offset: &mut u64,
) -> Result<Handshake>
where
    S: AsyncRead + Unpin,
{
    let mut reply = [0u8; 9];
    read_exact_clock(
        stream,
        &mut reply,
        clock,
        &format!("{where_}: read 9-byte handshake reply"),
    )
    .await?;
    *offset += 9;
    let mut packet = Packet::new(&reply);
    let code = packet.g1().context("handshake reply code")?;
    if code != 0 {
        return Ok(Handshake::Rejected(code));
    }
    Ok(Handshake::Token(
        packet.g8s().context("handshake server token")?,
    ))
}

/// How [`read_login_decision`] treats the step-98 replies of one attempt.
#[derive(Clone, Copy)]
pub(super) struct ReplyPolicy<'a> {
    /// Log/error prefix (`"lobby"` / `"world"`).
    pub(super) where_: &'a str,
    pub(super) is_lobby: bool,
    /// A lobby login that waits for the player (a device check may hold it):
    /// every lobby login but the relogin after a logout.
    pub(super) holds: bool,
    /// Reply 23 may still be retried (before the third retry).
    pub(super) can_retry_world_full: bool,
    /// Transfer replies are captured as `Attempt::Disallowed`.
    pub(super) capture_disallow: bool,
    /// The login presented a known social key: reply 35 means the server
    /// does not know it and the sign-on starts over.
    pub(super) renegotiates: bool,
}

/// What a successful step-98 decision leads to.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Decision {
    /// Reply 2: the profile (lobby) or the var blocks (world) follow.
    Proceed,
    /// Reply 15: the player-positions block of an in-place reconnect.
    Resume(Vec<u8>),
}

/// The poll interval of a login waiting on the player (parked or held).
pub(super) const LOGIN_WAIT_POLL: Duration = Duration::from_millis(20);

/// The first byte of `block` up to its first zero, as text (the page address
/// of reply 52).
pub(super) fn block_text(block: &[u8]) -> String {
    block
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| char::from(byte))
        .collect()
}

/// Write the packets the app queued for a waiting login connection.
pub(super) async fn flush_queued_packets<S>(stream: &mut S, progress: &LoginProgress) -> Result<()>
where
    S: AsyncWrite + Unpin,
{
    for packet in progress.take_outgoing() {
        stream
            .write_all(&progress.seal_outgoing(&packet)?)
            .await
            .context("send a queued packet while the login waits")?;
    }
    Ok(())
}

/// Read one byte while the login waits on the player (its clock is paused):
/// queued client packets go out and `login_continue` is not consulted, so the
/// wait ends only with the server's next byte or a dropped connection.
pub(super) async fn read_held_byte<S>(stream: &mut S, progress: &LoginProgress) -> Result<u8>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut byte = [0u8; 1];
    loop {
        flush_queued_packets(stream, progress).await?;
        match timeout(LOGIN_WAIT_POLL, stream.read(&mut byte)).await {
            Ok(Ok(0)) => bail!("the server closed the connection while the login waited"),
            Ok(Ok(_)) => return Ok(byte[0]),
            Ok(Err(error)) => return Err(error.into()),
            Err(_) => {}
        }
    }
}

/// Read step-98 reply bytes until the login either proceeds, must reconnect,
/// or fails fast. Returns `Done(Decision::Proceed)` on reply `2`,
/// `Done(Decision::Resume)` on reply 15 of a world login; `RetryAfter` for
/// reply 23 (and transport/timeout failures, mirroring the I/O error
/// reconnect arm); `Err` for fatal replies. Replies 42, 49 and 52 (waiting
/// lobby) consume their follow-up bytes and keep reading on the same
/// connection. The parked reply states are excluded from `loginwait`,
/// including subsequent success-block reads. Replies that keep the login
/// running are published through `progress` for the login screens.
/// Receiving reply 2 does not itself set the retained reply state. `offset` counts
/// server bytes consumed; `last_good` tracks the last non-fatal reply byte.
pub(super) async fn read_login_decision<S>(
    stream: &mut S,
    policy: ReplyPolicy<'_>,
    clock: &mut LoginClock,
    offset: &mut u64,
    last_good: &mut Option<u8>,
    progress: &LoginProgress,
) -> Result<Attempt<Decision>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let ReplyPolicy {
        where_,
        is_lobby,
        holds,
        can_retry_world_full,
        capture_disallow,
        renegotiates,
    } = policy;
    loop {
        // A waiting login reads with its clock paused, and keeps the
        // connection's queued packets flowing.
        let read = if clock.remaining().is_none() {
            read_held_byte(stream, progress).await
        } else {
            read_byte_clock(stream, clock, &format!("{where_}: read login reply code")).await
        };
        let code = match read {
            Ok(code) => code,
            Err(error) => {
                return Ok(retry_read(
                    &error,
                    format!("{where_}: reply read failed, reconnecting: {error:#}"),
                ));
            }
        };
        *offset += 1;
        let previous = *last_good;
        if code == 35 && renegotiates {
            *last_good = Some(code);
            return Ok(Attempt::Renegotiate);
        }
        match classify_login_code(code, holds) {
            LoginCode::Proceed => {
                *last_good = Some(code);
                return Ok(Attempt::Done(Decision::Proceed));
            }
            LoginCode::ConfirmRetry => {
                *last_good = Some(code);
                // Login step 103: the login screen shows its advertisement
                // countdown and calls `login_continue`. Step 110, where the
                // login goes on, reads nothing, so `loginwait` counts on to its
                // limit and the attempt reconnects.
                progress.set_interim_reply(1);
                clock.pause();
                while !progress.take_resume() {
                    sleep(LOGIN_WAIT_POLL).await;
                }
                clock.resume();
                sleep(clock.remaining().unwrap_or(Duration::ZERO)).await;
                return Ok(Attempt::RetryAfter {
                    after_ms: 0,
                    why: format!(
                        "{where_}: reply 1 continued; the login step after it reads nothing, \
                         loginwait expired"
                    ),
                    timeout: true,
                });
            }
            LoginCode::WorldFull => {
                *last_good = Some(code);
                if !can_retry_world_full {
                    return Err(anyhow!(
                        "{where_}: reply 23 after the third retry is terminal"
                    ));
                }
                return Ok(retry_after(
                    0,
                    format!("{where_}: reply 23, immediate reconnect"),
                ));
            }
            LoginCode::HopWait => {
                *last_good = Some(code);
                let hop = match read_byte_clock(
                    stream,
                    clock,
                    &format!("{where_}: read reply-21 hop timer"),
                )
                .await
                {
                    Ok(hop) => hop,
                    Err(error) => {
                        return Ok(retry_read(
                            &error,
                            format!("{where_}: hop-timer read failed, reconnecting: {error:#}"),
                        ));
                    }
                };
                *offset += 1;
                let after_ms = hop_retry_after_ms(hop);
                return Err(anyhow::Error::new(LoginReplyState {
                    reply: 21,
                    hoptime: i32::try_from(after_ms).unwrap_or(i32::MAX),
                    ban_duration: 0,
                }));
            }
            LoginCode::Queued => {
                *last_good = Some(code);
                // The login enters step 215 before waiting for the queue
                // position; the loginwait predicate excludes this read.
                clock.pause();
                let position = match read_u16_clock(
                    stream,
                    clock,
                    &format!("{where_}: read reply-42 queue position"),
                )
                .await
                {
                    Ok(position) => position,
                    Err(error) => {
                        return Ok(retry_read(
                            &error,
                            format!(
                                "{where_}: queue-position read failed, reconnecting: {error:#}"
                            ),
                        ));
                    }
                };
                *offset += 2;
                progress.set_queue_position(i32::from(position));
                progress.set_interim_reply(42);
                log::warn!("[client910] {where_}: reply 42 queued at position {position}; paused loginwait");
                continue;
            }
            LoginCode::LobbyHold => {
                *last_good = Some(code);
                progress.set_interim_reply(49);
                clock.pause();
                continue;
            }
            LoginCode::UrlBlock => {
                *last_good = Some(code);
                let size = match read_u16_clock(
                    stream,
                    clock,
                    &format!("{where_}: read reply-52 block size"),
                )
                .await
                {
                    Ok(size) => size,
                    Err(error) => {
                        return Ok(retry_read(
                            &error,
                            format!("{where_}: reply-52 size read failed, reconnecting: {error:#}"),
                        ));
                    }
                };
                let mut block = vec![0u8; usize::from(size)];
                if let Err(error) = read_exact_clock(
                    stream,
                    &mut block,
                    clock,
                    &format!("{where_}: read reply-52 block"),
                )
                .await
                {
                    return Ok(retry_read(
                        &error,
                        format!("{where_}: reply-52 block read failed, reconnecting: {error:#}"),
                    ));
                }
                *offset += 2 + u64::from(size);
                progress.unmask_reply_block(&mut block);
                progress.push_url(block_text(&block));
                progress.set_interim_reply(52);
                if holds {
                    clock.pause();
                    log::info!(
                        "[client910] {where_}: reply 52 with a {}-byte page address, back to step 98",
                        block.len()
                    );
                    continue;
                }
                return Err(anyhow::Error::new(LoginReplyState {
                    reply: 52,
                    hoptime: 0,
                    ban_duration: 0,
                })
                .context(format!(
                    "{where_}: reply 52 with a {}-byte page address at stream offset {offset} \
                     (last good reply {}): the login ends here",
                    block.len(),
                    previous_name(previous)
                )));
            }
            LoginCode::AltSuccess if !is_lobby => {
                *last_good = Some(code);
                // Step 204 framing, consumed exactly (u16 size + block).
                let size = match read_u16_clock(
                    stream,
                    clock,
                    &format!("{where_}: read reply-15 block size"),
                )
                .await
                {
                    Ok(size) => size,
                    Err(error) => {
                        return Ok(retry_read(
                            &error,
                            format!("{where_}: reply-15 size read failed, reconnecting: {error:#}"),
                        ));
                    }
                };
                let mut block = vec![0u8; usize::from(size)];
                if let Err(error) = read_exact_clock(
                    stream,
                    &mut block,
                    clock,
                    &format!("{where_}: read reply-15 block"),
                )
                .await
                {
                    return Ok(retry_read(
                        &error,
                        format!("{where_}: reply-15 block read failed, reconnecting: {error:#}"),
                    ));
                }
                *offset += 2 + u64::from(size);
                return Ok(Attempt::Done(Decision::Resume(block)));
            }
            LoginCode::Banned => {
                let duration = match read_u32_clock(
                    stream,
                    clock,
                    &format!("{where_}: read reply-53 ban duration"),
                )
                .await
                {
                    Ok(duration) => duration,
                    Err(error) => {
                        return Ok(retry_read(
                            &error,
                            format!("{where_}: ban-duration read failed, reconnecting: {error:#}"),
                        ));
                    }
                };
                *offset += 4;
                return Err(anyhow::Error::new(LoginReplyState {
                    reply: 53,
                    hoptime: 0,
                    ban_duration: duration as i32,
                }));
            }
            // Reply 15 on a lobby login (an in-place reconnect has no lobby
            // to return to) ends the login like any unexpected reply.
            LoginCode::AltSuccess | LoginCode::Fatal => {
                if capture_disallow && matches!(code, 29 | 45) {
                    let result = match read_byte_clock(
                        stream,
                        clock,
                        &format!("{where_}: read transfer disallow result"),
                    )
                    .await
                    {
                        Ok(result) => result,
                        Err(error) => {
                            return Ok(retry_read(
                                &error,
                                format!(
                                    "{where_}: transfer disallow result read failed, reconnecting: {error:#}"
                                ),
                            ));
                        }
                    };
                    *offset += 1;
                    let trigger = if code == 45 {
                        match read_u16_clock(
                            stream,
                            clock,
                            &format!("{where_}: read transfer disallow trigger"),
                        )
                        .await
                        {
                            Ok(trigger) => {
                                *offset += 2;
                                trigger
                            }
                            Err(error) => {
                                return Ok(retry_read(
                                    &error,
                                    format!(
                                        "{where_}: transfer disallow trigger read failed, reconnecting: {error:#}"
                                    ),
                                ));
                            }
                        }
                    } else {
                        0
                    };
                    *last_good = Some(code);
                    return Ok(Attempt::Disallowed {
                        reply: code,
                        result,
                        trigger,
                    });
                }
                // The real reply (e.g. 5 "already logged in") is kept for the
                // login/lobby scripts.
                return Err(anyhow::Error::new(LoginReplyState {
                    reply: code as i32,
                    hoptime: 0,
                    ban_duration: 0,
                })
                .context(format!(
                    "{where_}: fatal login reply {code} at stream offset {offset} \
                     (last good reply {})",
                    previous_name(previous)
                )));
            }
        }
    }
}

pub(super) fn previous_name(previous: Option<u8>) -> String {
    match previous {
        Some(code) => code.to_string(),
        None => "none".to_string(),
    }
}

/// The two login kinds (the lobby and game connections). They
/// share the connection, handshake and attempt loop and differ in the login
/// packet, the reply policy and the success block.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoginKind {
    Lobby,
    World,
}

impl LoginKind {
    /// The log/error prefix.
    pub(super) fn label(self) -> &'static str {
        match self {
            LoginKind::Lobby => "lobby",
            LoginKind::World => "world",
        }
    }
}

/// The lobby login (`LOBBYLOGIN`): up to
/// [`LOGIN_MAX_ATTEMPTS`] connections, then the lobby socket and profile.
pub async fn login_lobby(params: &LoginParams) -> Result<(WireStream<TcpStream>, LoginOk)> {
    login_with_retries(LoginKind::Lobby, params).await
}

/// The game login (`GAMELOGIN`, then `GAMELOGIN_CONTINUE`): up
/// to [`LOGIN_MAX_ATTEMPTS`] connections, then the world socket and the
/// continuation block's fields.
pub async fn login_world(params: &LoginParams) -> Result<(WireStream<TcpStream>, LoginOk)> {
    login_with_retries(LoginKind::World, params).await
}

/// The attempt loop of both kinds: retry transport failures, timeouts and
/// reply 23 while attempts remain, then report -5/-4. A social sign-on that
/// presents a key the server no longer knows (reply 35) starts its sign-on
/// over on a fresh connection without using up an attempt.
pub(super) async fn login_with_retries(
    kind: LoginKind,
    params: &LoginParams,
) -> Result<(WireStream<TcpStream>, LoginOk)> {
    let where_ = kind.label();
    let mut last_why = String::from("not attempted");
    let mut last_timeout = false;
    // A social sign-on with no key yet negotiates one first.
    let mut negotiate = params
        .sso
        .as_ref()
        .is_some_and(|sso| sso.social_key.is_none());
    let mut attempt = 0;
    while attempt < LOGIN_MAX_ATTEMPTS {
        let outcome = match open_connection(params, attempt, where_)
            .await
            .map(Attempt::done)
        {
            Ok(Ok(connection)) => match kind {
                LoginKind::Lobby => lobby_login_steps(connection, params, attempt, negotiate).await,
                LoginKind::World => world_login_steps(connection, params, attempt, negotiate).await,
            },
            Ok(Err(other)) => Ok(other),
            Err(error) => Err(error),
        };
        match outcome {
            Ok(Attempt::Done((stream, ok))) => {
                return Ok((WireStream::new(stream, params.progress.take_cipher()), ok));
            }
            Ok(Attempt::Renegotiate) => {
                log::info!("[client910] {where_}: the server does not know the social key; signing on again");
                negotiate = true;
                continue;
            }
            Ok(Attempt::RetryAfter {
                after_ms,
                why,
                timeout,
            }) => {
                log::warn!(
                    "[client910] WARN {where_} login attempt {}/{}: {why}",
                    attempt + 1,
                    LOGIN_MAX_ATTEMPTS
                );
                last_why = why;
                last_timeout = timeout;
                if after_ms > 0 {
                    sleep(Duration::from_millis(after_ms)).await;
                }
            }
            Ok(Attempt::Disallowed {
                reply,
                result,
                trigger,
            }) => {
                if kind == LoginKind::Lobby {
                    unreachable!("lobby login does not capture transfer replies")
                }
                return Err(anyhow::Error::new(LoginTransferFailure {
                    reply: i32::from(reply),
                    disallow_result: i32::from(result),
                    disallow_trigger: i32::from(trigger),
                }));
            }
            Err(error) => return Err(error),
        }
        attempt += 1;
    }
    let exhausted = LoginAttemptsExhausted::from_last_attempt(last_timeout);
    Err(anyhow::Error::new(exhausted).context(format!(
        "{where_} login failed after {LOGIN_MAX_ATTEMPTS} attempts (last: {last_why}; reply {})",
        exhausted.reply
    )))
}

/// One attempt's connection after steps 14/35: the stream past the 9-byte
/// `INIT_GAME_CONNECTION` reply, the attempt's `loginwait` clock, the server
/// bytes consumed so far and the session token.
pub(super) struct Handshaken<S> {
    stream: S,
    clock: LoginClock,
    offset: u64,
    server_token: i64,
}

/// Step 14 of one attempt: connect to `params.host:params.port`, the only
/// socket the login opens (the transport half; everything after it runs
/// over any byte stream), then [`handshake`].
pub(super) async fn open_connection(
    params: &LoginParams,
    attempt: u32,
    where_: &str,
) -> Result<Attempt<Handshaken<TcpStream>>> {
    // Connect + handshake are steps 14/35 (< 98), so the first attempt gets
    // the 500-update threshold; step 98 raises it to 2000 updates.
    // Non-social: social_late = false.
    let clock = LoginClock::new(attempt == 0);
    let addr = format!("{}:{}", params.host, params.port);
    let stream = match timeout(
        clock.remaining().unwrap_or(Duration::ZERO),
        TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => {
            return Ok(retry_after(
                0,
                format!("{where_}: connect {addr} failed, reconnecting: {error:#}"),
            ));
        }
        Err(_) => {
            return Ok(retry_after(
                0,
                format!(
                    "{where_}: connect {addr} timed out after {} ms, reconnecting",
                    stage_timeout_duration(attempt == 0, false).as_millis()
                ),
            ));
        }
    };
    handshake(stream, clock, where_).await
}

/// Step 35: send `INIT_GAME_CONNECTION` and read the 9-byte reply.
pub(super) async fn handshake<S>(
    mut stream: S,
    clock: LoginClock,
    where_: &str,
) -> Result<Attempt<Handshaken<S>>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    if let Err(error) = stream.write_all(&init_game_connection_packet()).await {
        return Ok(retry_after(
            0,
            format!("{where_}: send INIT_GAME_CONNECTION failed, reconnecting: {error:#}"),
        ));
    }
    let mut offset = 0u64;
    let server_token = match read_handshake_token(&mut stream, where_, &clock, &mut offset).await {
        Ok(Handshake::Token(token)) => token,
        // The server's code is the login's reply (login step 35): the login
        // screens show its message.
        Ok(Handshake::Rejected(code)) => {
            return Err(anyhow::Error::new(LoginReplyState {
                reply: i32::from(code),
                hoptime: 0,
                ban_duration: 0,
            })
            .context(format!(
                "{where_}: handshake rejected with code {code} at stream offset 0"
            )));
        }
        Err(error) => {
            return Ok(retry_read(
                &error,
                format!("{where_}: handshake failed, reconnecting: {error:#}"),
            ));
        }
    };
    Ok(Attempt::Done(Handshaken {
        stream,
        clock,
        offset,
        server_token,
    }))
}

/// The social sign-on's opening on an established connection (login steps
/// 276 to 70): announce the network, open the page the server names for the
/// player to sign in on, wait for the server to confirm the sign-on and read
/// the account's key and name word, which the caller's later logins present.
pub(super) async fn social_negotiate<S>(
    stream: &mut S,
    kind: ConnectionKind,
    params: &LoginParams,
    server_token: i64,
    clock: &mut LoginClock,
    offset: &mut u64,
) -> Result<Attempt<Option<SessionSeeds>>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let where_ = match kind {
        ConnectionKind::Lobby => "lobby",
        ConnectionKind::World => "world",
    };
    // Deterministic build: a failure here is fatal, never retried.
    let (packet, seeds) = seal_social_connection_packet(kind, params, server_token)?;
    if let Err(error) = stream.write_all(&packet).await {
        return Ok(retry_after(
            0,
            format!(
                "{where_}: send INIT_SOCIAL_NETWORK_CONNECTION failed, reconnecting: {error:#}"
            ),
        ));
    }
    // Steps 40 and 58: the page address, a size and a `gjstr2` string.
    let size =
        match read_u16_clock(stream, clock, &format!("{where_}: read sign-on page size")).await {
            Ok(size) => size,
            Err(error) => {
                return Ok(retry_read(
                    &error,
                    format!("{where_}: sign-on page size read failed, reconnecting: {error:#}"),
                ));
            }
        };
    let mut block = vec![0u8; usize::from(size)];
    if let Err(error) = read_exact_clock(
        stream,
        &mut block,
        clock,
        &format!("{where_}: read sign-on page"),
    )
    .await
    {
        return Ok(retry_read(
            &error,
            format!("{where_}: sign-on page read failed, reconnecting: {error:#}"),
        ));
    }
    *offset += 2 + u64::from(size);
    if let Some(seeds) = &seeds {
        let end = block.len();
        rs910_protocol::tiny_cipher::decrypt_range(&mut block, 0, end, seeds);
    }
    let page = match block.split_first() {
        Some((0, text)) => block_text(text),
        _ => bail!("{where_}: the sign-on page address has no version byte"),
    };
    if !page.is_empty() {
        params.progress.push_url(page);
    }
    // Step 64: the player signs in; the server sends a 1 when they have.
    clock.enter_social();
    loop {
        match read_byte_clock(stream, clock, &format!("{where_}: wait for the sign-on")).await {
            Ok(1) => break,
            Ok(_) => *offset += 1,
            Err(error) => {
                return Ok(retry_read(
                    &error,
                    format!("{where_}: sign-on wait failed, reconnecting: {error:#}"),
                ));
            }
        }
    }
    *offset += 1;
    // Step 70: the account's key and name word.
    let mut account = [0u8; 16];
    if let Err(error) = read_exact_clock(
        stream,
        &mut account,
        clock,
        &format!("{where_}: read sign-on account"),
    )
    .await
    {
        return Ok(retry_read(
            &error,
            format!("{where_}: sign-on account read failed, reconnecting: {error:#}"),
        ));
    }
    *offset += 16;
    if let Some(seeds) = &seeds {
        rs910_protocol::tiny_cipher::decrypt_range(&mut account, 0, 16, seeds);
    }
    let key = i64::from_be_bytes(account[..8].try_into().expect("8 bytes"));
    let name = i64::from_be_bytes(account[8..].try_into().expect("8 bytes"));
    params.progress.set_social(key, name);
    Ok(Attempt::Done(seeds))
}

/// The lobby login after the handshake: `LOBBYLOGIN` (or, after a social
/// sign-on, `SOCIAL_NETWORK_LOGIN`), the step-98 decision and the profile
/// block.
pub(super) async fn lobby_login_steps<S>(
    connection: Handshaken<S>,
    params: &LoginParams,
    attempt: u32,
    negotiate: bool,
) -> Result<Attempt<(S, LoginOk)>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    const WHERE: &str = "lobby";
    let Handshaken {
        mut stream,
        mut clock,
        mut offset,
        server_token,
    } = connection;

    let (packet, seeds) = if negotiate {
        let negotiated = social_negotiate(
            &mut stream,
            ConnectionKind::Lobby,
            params,
            server_token,
            &mut clock,
            &mut offset,
        )
        .await?;
        let seeds = match negotiated.done() {
            Ok(seeds) => seeds,
            Err(other) => return Ok(other),
        };
        (
            seal_social_login_packet(ConnectionKind::Lobby, params, seeds)?,
            seeds,
        )
    } else {
        // Deterministic build: a failure here is fatal, never retried.
        seal_lobby_login_packet(params, server_token)?
    };
    if let Err(error) = stream.write_all(&packet).await {
        return Ok(retry_after(
            0,
            format!("{WHERE}: send the login failed, reconnecting: {error:#}"),
        ));
    }
    params.progress.install_cipher(seeds.map(WireCipher::new));
    let mut last_good: Option<u8> = None;
    clock.enter_step98();
    match read_login_decision(
        &mut stream,
        ReplyPolicy {
            where_: WHERE,
            is_lobby: true,
            holds: !params.relogin,
            can_retry_world_full: attempt < 3,
            capture_disallow: false,
            renegotiates: params.sso.is_some() && !negotiate,
        },
        &mut clock,
        &mut offset,
        &mut last_good,
        &params.progress,
    )
    .await?
    {
        Attempt::Done(Decision::Proceed) => {}
        Attempt::Done(Decision::Resume(_)) => {
            unreachable!("a lobby login has no in-place reconnect")
        }
        other => {
            let Err(passed_on) = other.done::<(S, LoginOk)>() else {
                unreachable!("Done is handled above")
            };
            return Ok(passed_on);
        }
    }
    let size = match read_byte_clock(&mut stream, &clock, "lobby: read login reply size").await {
        Ok(size) => size,
        Err(error) => {
            return Ok(retry_read(
                &error,
                format!("{WHERE}: reply-block size read failed, reconnecting: {error:#}"),
            ));
        }
    };
    // NOTE: `offset` is intentionally not advanced past this final success
    // block: nothing downstream observes the login cursor after `Done`.
    let mut payload = vec![0u8; usize::from(size)];
    if let Err(error) = read_exact_clock(
        &mut stream,
        &mut payload,
        &clock,
        "lobby: read login reply block",
    )
    .await
    {
        return Ok(retry_read(
            &error,
            format!("{WHERE}: reply-block read failed, reconnecting: {error:#}"),
        ));
    }
    params.progress.unmask_auth_key(&mut payload);
    let (profile, lobby) = parse_lobby_profile(&payload)?;
    Ok(Attempt::Done((
        stream,
        LoginOk {
            server_varcs: vec![],
            pid: None,
            server_token,
            profile,
            lobby,
            resume: None,
        },
    )))
}

/// Parse the post-login lobby profile block.
/// The first byte is the auth-preferences marker; the optional four bytes are
/// consumed here even though the preferences cache is owned by the client
/// startup path. The remaining fields are retained by the VM owner or are
/// deliberately consumed to keep the stream cursor aligned.
pub(super) fn parse_lobby_profile(block: &[u8]) -> Result<(LoginProfile, LobbyProfile)> {
    let mut packet = Packet::new(block);
    if packet
        .g1()
        .context("lobby profile missing auth-preferences flag")?
        == 1
    {
        let _ = packet
            .gdata(4, "lobby auth preferences")
            .context("lobby profile auth preferences")?;
    }
    let staff_mod_level = i32::from(packet.g1().context("lobby profile missing staffModLevel")?);
    let player_mod_level = i32::from(
        packet
            .g1()
            .context("lobby profile missing playerModLevel")?,
    );
    let dob_verified = packet.g1().context("lobby profile missing dobVerified")? == 1;
    let lobby_dob = packet.g3().context("lobby profile missing lobbyDOB")? as i32;
    let lobby_dob = if lobby_dob > 0x7F_FFFF {
        lobby_dob - 0x100_0000
    } else {
        lobby_dob
    };
    let _gender = packet.g1().context("lobby profile missing gender")?;
    let player_is_quickchat = packet
        .g1()
        .context("lobby profile missing playerIsQuickChat")?
        == 1;
    let _reserved_flag = packet.g1().context("lobby profile missing reserved flag")?;
    let lobby_membership = packet.g8s().context("lobby profile missing membership")?;
    let lobby_membership_delay = read_u40(&mut packet, "lobby membership delay")?;
    let flags = packet
        .g1()
        .context("lobby profile missing membership flags")?;
    let lobby_jcoins_balance = packet.g4s().context("lobby profile missing jcoins")?;
    let lobby_loyalty_balance = packet.g4s().context("lobby profile missing loyalty")?;
    let lobby_recovery_day = i32::from(packet.g2().context("lobby profile missing recovery day")?);
    let lobby_unread_messages = i32::from(
        packet
            .g2()
            .context("lobby profile missing unread messages")?,
    );
    let lobby_last_login_day = i32::from(
        packet
            .g2()
            .context("lobby profile missing last login day")?,
    );
    let lobby_player_host = packet.g4s().context("lobby profile missing player host")?;
    let lobby_email_status = i32::from(packet.g1().context("lobby profile missing email status")?);
    let lobby_cc_expiry = i32::from(packet.g2().context("lobby profile missing cc expiry")?);
    let lobby_grace_expiry = i32::from(packet.g2().context("lobby profile missing grace expiry")?);
    let lobby_dob_requested = packet.g1().context("lobby profile missing dob requested")? == 1;
    let lobby_player_name = packet
        .gjstr2()
        .context("lobby profile missing player name")?;
    let lobby_members_stats =
        i32::from(packet.g1().context("lobby profile missing members stats")?);
    let lobby_play_age = packet.g4s().context("lobby profile missing play age")?;
    let world_node = packet.g2().context("lobby profile missing target world")?;
    let world_host = packet
        .gjstr2()
        .context("lobby profile missing target host")?;
    let world_port = packet.g2().context("lobby profile missing target port")?;
    let world_port2 = packet.g2().context("lobby profile missing target port2")?;
    Ok((
        LoginProfile {
            logged_in_members: false,
            player_is_members: flags & 0x1 != 0,
            player_is_quickchat,
            logged_in_quickchat: false,
            dob_verified,
            lobby_dob,
            staff_mod_level,
            player_mod_level,
            owner: None,
            server_clock: None,
        },
        LobbyProfile {
            membership: lobby_membership,
            membership_delay: lobby_membership_delay,
            membership_flag: flags & 0x2 != 0,
            unread_messages: lobby_unread_messages,
            recovery_day: lobby_recovery_day,
            jcoins_balance: lobby_jcoins_balance,
            loyalty_balance: lobby_loyalty_balance,
            last_login_day: lobby_last_login_day,
            player_host: lobby_player_host,
            email_status: lobby_email_status,
            cc_expiry: lobby_cc_expiry,
            grace_expiry: lobby_grace_expiry,
            dob_requested: lobby_dob_requested,
            members_stats: lobby_members_stats,
            play_age: lobby_play_age,
            player_name: lobby_player_name,
            world_id: (world_node != u16::MAX).then_some(world_node),
            world_host,
            world_port,
            world_port2,
        },
    ))
}

pub(super) fn read_u40(packet: &mut Packet<'_>, what: &'static str) -> Result<i64> {
    let bytes = packet.gdata(5, what)?;
    Ok(bytes
        .into_iter()
        .fold(0_i64, |value, byte| (value << 8) | i64::from(byte)))
}

/// `GAMELOGIN_CONTINUE` reply fields the client keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContinueBlock {
    /// The player uid (the server writes the player index here).
    pub pid: u16,
    /// The flags, the owner name (`gjstr` after the members flag) and the `g6`
    /// server clock, whose difference from the local monotonic clock the client
    /// keeps; `owner` and `server_clock` are always `Some`.
    pub profile: LoginProfile,
}

/// Parse the `GAMELOGIN_CONTINUE` reply block (login step 157):
/// ```text
/// auth preferences: g1 flag (== 1 -> 4 x gIsaac1 auth key)
/// g1 staffModLevel | g1 playerModLevel | g1 dobVerified | g1 playerIsQuickChat
/// g1 reserved | g1 loggedInQuickChat | g2 currentPlayerUid | g1 playerIsMembers
/// g3s lobbyDOB | g1 loggedInMembers | gjstr owner | g6 server clock
/// ```
pub(super) fn parse_continue_block(block: &[u8]) -> Result<ContinueBlock> {
    let mut packet = Packet::new(block);
    let what = "GAMELOGIN_CONTINUE block";
    // Auth preferences: only a flag of exactly 1 carries the 4-byte key (the
    // retained client keeps no auth-preference store for world logins).
    if packet
        .g1()
        .context("GAMELOGIN_CONTINUE block missing auth flag")?
        == 1
    {
        let _ = packet.gdata(4, "GAMELOGIN_CONTINUE auth preferences")?;
    }
    let mut g1 = |field: &'static str| {
        packet
            .g1()
            .with_context(|| format!("{what} missing {field}"))
    };
    let staff_mod_level = i32::from(g1("staffModLevel")?);
    let player_mod_level = i32::from(g1("playerModLevel")?);
    let dob_verified = g1("dobVerified")? == 1;
    let player_is_quickchat = g1("playerIsQuickChat")? == 1;
    let _reserved_flag = g1("reservedFlag")?;
    let logged_in_quickchat = g1("loggedInQuickChat")? == 1;
    let pid = packet
        .g2()
        .context("GAMELOGIN_CONTINUE block missing pid")?;
    let player_is_members = packet
        .g1()
        .context("GAMELOGIN_CONTINUE block missing playerIsMembers")?
        == 1;
    let dob = packet
        .g3()
        .context("GAMELOGIN_CONTINUE block missing lobbyDOB")? as i32;
    // Packet.g3s: sign-extend the 24-bit value.
    let lobby_dob = if dob > 0x7F_FFFF {
        dob - 0x100_0000
    } else {
        dob
    };
    let logged_in_members = packet
        .g1()
        .context("GAMELOGIN_CONTINUE block missing loggedInMembers")?
        == 1;
    let owner = packet
        .gjstr()
        .context("GAMELOGIN_CONTINUE block missing owner")?;
    // g6: g2 high half, g4 low half, unsigned.
    let clock = packet.gdata(6, "GAMELOGIN_CONTINUE server clock")?;
    let server_clock = clock
        .into_iter()
        .fold(0_i64, |value, byte| (value << 8) | i64::from(byte));
    Ok(ContinueBlock {
        pid,
        profile: LoginProfile {
            logged_in_members,
            player_is_members,
            player_is_quickchat,
            logged_in_quickchat,
            dob_verified,
            lobby_dob,
            staff_mod_level,
            player_mod_level,
            owner: Some(owner),
            server_clock: Some(server_clock),
        },
    })
}

/// The game login after the handshake: `GAMELOGIN`, the step-98 decision,
/// the var blocks (steps 256/268), `GAMELOGIN_CONTINUE`, the second decision
/// and the continuation block.
pub(super) async fn world_login_steps<S>(
    connection: Handshaken<S>,
    params: &LoginParams,
    attempt: u32,
    negotiate: bool,
) -> Result<Attempt<(S, LoginOk)>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    const WHERE: &str = "world";
    // Same cumulative update budget as the lobby attempt.
    let Handshaken {
        mut stream,
        mut clock,
        mut offset,
        server_token,
    } = connection;

    let (packet, seeds) = if negotiate {
        let negotiated = social_negotiate(
            &mut stream,
            ConnectionKind::World,
            params,
            server_token,
            &mut clock,
            &mut offset,
        )
        .await?;
        let seeds = match negotiated.done() {
            Ok(seeds) => seeds,
            Err(other) => return Ok(other),
        };
        (
            seal_social_login_packet(ConnectionKind::World, params, seeds)?,
            seeds,
        )
    } else {
        // Deterministic build: a failure here is fatal, never retried.
        seal_game_login_packet(params, server_token)?
    };
    if let Err(error) = stream.write_all(&packet).await {
        return Ok(retry_after(
            0,
            format!("{WHERE}: send the login failed, reconnecting: {error:#}"),
        ));
    }
    params.progress.install_cipher(seeds.map(WireCipher::new));
    let mut last_good: Option<u8> = None;
    clock.enter_step98();
    match read_login_decision(
        &mut stream,
        ReplyPolicy {
            where_: WHERE,
            is_lobby: false,
            holds: false,
            can_retry_world_full: attempt < 3,
            capture_disallow: false,
            renegotiates: params.sso.is_some() && !negotiate,
        },
        &mut clock,
        &mut offset,
        &mut last_good,
        &params.progress,
    )
    .await?
    {
        Attempt::Done(Decision::Proceed) => {}
        // Reply 15: the server kept the character in its world. Nothing
        // follows the player-positions block; the client's own state carries on.
        Attempt::Done(Decision::Resume(positions)) => {
            return Ok(Attempt::Done((
                stream,
                LoginOk {
                    server_varcs: Vec::new(),
                    pid: None,
                    server_token,
                    profile: LoginProfile::default(),
                    lobby: LobbyProfile::default(),
                    resume: Some(positions),
                },
            )));
        }
        Attempt::Disallowed { .. } => {
            unreachable!("initial world login does not capture transfer replies")
        }
        other => {
            let Err(passed_on) = other.done::<(S, LoginOk)>() else {
                unreachable!("Done is handled above")
            };
            return Ok(passed_on);
        }
    }
    // Login steps 256/268: the final-block flag is FIRST, followed
    // by typed values. Cache types are decoded later by the game owner.
    let mut server_varcs = Vec::new();
    loop {
        let var_size =
            match read_u16_clock(&mut stream, &clock, "world: read var-update size").await {
                Ok(size) => size,
                Err(error) => {
                    return Ok(retry_read(
                        &error,
                        format!("{WHERE}: var-update size read failed, reconnecting: {error:#}"),
                    ))
                }
            };
        offset += 2;
        let mut block = vec![0; usize::from(var_size)];
        if let Err(error) = read_exact_clock(
            &mut stream,
            &mut block,
            &clock,
            "world: read var-update block",
        )
        .await
        {
            return Ok(retry_read(
                &error,
                format!("{WHERE}: var-update block read failed, reconnecting: {error:#}"),
            ));
        }
        offset += u64::from(var_size);
        let (&last, values) = block.split_first().context("empty world variable block")?;
        server_varcs.extend_from_slice(values);
        if last == 1 {
            break;
        }
    }

    if let Err(error) = stream.write_all(&game_login_continue_packet()).await {
        return Ok(retry_after(
            0,
            format!("{WHERE}: send GAMELOGIN_CONTINUE failed, reconnecting: {error:#}"),
        ));
    }
    // Second step-98 decision after GAMELOGIN_CONTINUE
    // (step 138, then 141/157).
    match read_login_decision(
        &mut stream,
        ReplyPolicy {
            where_: WHERE,
            is_lobby: false,
            holds: false,
            can_retry_world_full: attempt < 3,
            capture_disallow: true,
            renegotiates: false,
        },
        &mut clock,
        &mut offset,
        &mut last_good,
        &params.progress,
    )
    .await
    .map_err(|error| error.context(LateLoginFailure))?
    {
        Attempt::Done(Decision::Proceed) => {}
        Attempt::Done(Decision::Resume(_)) => {
            bail!("{WHERE}: reply 15 after GAMELOGIN_CONTINUE is not an in-place reconnect")
        }
        Attempt::Disallowed {
            reply,
            result,
            trigger,
        } => {
            return Err(anyhow::Error::new(LoginTransferFailure {
                reply: i32::from(reply),
                disallow_result: i32::from(result),
                disallow_trigger: i32::from(trigger),
            })
            .context(LateLoginFailure));
        }
        other => {
            let Err(passed_on) = other.done::<(S, LoginOk)>() else {
                unreachable!("Done is handled above")
            };
            return Ok(passed_on);
        }
    }
    let size = match read_byte_clock(&mut stream, &clock, "world: read GAMELOGIN_CONTINUE size")
        .await
    {
        Ok(size) => size,
        Err(error) => {
            return Ok(retry_read(
                &error,
                format!("{WHERE}: GAMELOGIN_CONTINUE size read failed, reconnecting: {error:#}"),
            ));
        }
    };
    // NOTE: `offset` is intentionally not advanced past this final success
    // block: nothing downstream observes the login cursor after `Done`.
    let mut block = vec![0u8; usize::from(size)];
    if let Err(error) = read_exact_clock(
        &mut stream,
        &mut block,
        &clock,
        "world: read GAMELOGIN_CONTINUE block",
    )
    .await
    {
        return Ok(retry_read(
            &error,
            format!("{WHERE}: GAMELOGIN_CONTINUE block read failed, reconnecting: {error:#}"),
        ));
    }
    // NOTE: `offset` is intentionally not advanced past this final success
    // block: nothing downstream observes the login cursor after `Done`.
    params.progress.unmask_auth_key(&mut block);
    let ContinueBlock { pid, profile } = parse_continue_block(&block)?;
    Ok(Attempt::Done((
        stream,
        LoginOk {
            server_varcs,
            pid: Some(pid),
            server_token,
            profile,
            lobby: LobbyProfile::default(),
            resume: None,
        },
    )))
}
