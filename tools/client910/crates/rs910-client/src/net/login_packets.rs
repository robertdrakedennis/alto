//! Pure login packet composition and cryptographic sealing.

use anyhow::{Context, Result};

use native910::packet::ByteWriter;

use rs910_core::hardware::Hardware;

use crate::login_crypto::{LoginCrypto, LoginSeeds};

use crate::proto;

use rs910_protocol::wire_cipher::SessionSeeds;

use super::{AuthOptions, ClientReport, LauncherReport, LoginParams, ARCHIVE_CHECKSUM_WORDS};

// ---------------------------------------------------------------------------
// Small writers for shapes `native910::packet` does not provide.
// (`ByteWriter` covers p1/p2/p4/p8/pdata/pjstr; these compose it, they do not
// reimplement the packet layer.)
// ---------------------------------------------------------------------------

/// 3-byte big-endian int (`p3`): `[(v >> 16), (v >> 8), v]`.
pub(super) fn p3(writer: &mut ByteWriter, value: u32) {
    writer.p1((value >> 16) as u8);
    writer.p2((value & 0xFFFF) as u16);
}

/// `pjstr2`: leading zero version byte + NUL-terminated string.
/// Server reads these with `gjstr2` (see `Packet.ts`), so an empty string
/// encodes as 2 bytes (`[0x00, 0x00]`), not 1.
pub(super) fn pjstr2(writer: &mut ByteWriter, value: &str) -> Result<()> {
    writer.p1(0);
    writer.pjstr(value)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Login block builders (pure: no socket needed, fully unit-testable).
// ---------------------------------------------------------------------------

/// The TOTP type byte the login writes: the type's wire id, not its position
/// in the original's type table.
mod totp_id {
    /// A stored trust token follows (`p4`). This client keeps none.
    #[allow(dead_code, reason = "wire id of a type this client never sends")]
    pub const TOKEN_FOUND: u8 = 0;
    /// A six-digit code entered with "don't trust this computer" ticked.
    pub const CODE_DONT_TRUST: u8 = 1;
    /// No stored token and no code: four reserved bytes follow.
    pub const NOT_FOUND: u8 = 2;
    /// A six-digit code entered with "trust this computer" ticked.
    pub const CODE_TRUST: u8 = 3;
}

/// The start of every RSA block: the marker, this connection's four seeds
/// (zero stubs without encryption) and the server token. A reconnect then
/// repeats the previous connection's seeds.
pub(super) fn push_rsa_start(
    writer: &mut ByteWriter,
    server_token: i64,
    seeds: &LoginSeeds,
    reconnect: bool,
) {
    writer.p1(10);
    for seed in seeds.current {
        writer.p4s(seed);
    }
    writer.p8s(server_token);
    if reconnect {
        for seed in seeds.previous {
            writer.p4s(seed);
        }
    }
}

/// The authenticator block: a typed marker and its payload.
pub(super) fn push_authenticator(writer: &mut ByteWriter, auth: &AuthOptions) -> Result<()> {
    if auth.new_auth_preference.len() == 6 {
        let preference = auth
            .new_auth_preference
            .parse::<u32>()
            .map_err(|_| anyhow::anyhow!("auth preference must be six digits"))?;
        writer.p1(if auth.auth_dont_trust {
            totp_id::CODE_DONT_TRUST
        } else {
            totp_id::CODE_TRUST
        });
        p3(writer, preference);
        writer.p1(0);
    } else {
        writer.p1(totp_id::NOT_FOUND);
        writer.p4s(0);
    }
    Ok(())
}

/// The RSA block of a game or lobby login (unencrypted here: [`LoginCrypto`]
/// encrypts the region this writes).
///
/// A fresh login carries the credentials: the authenticator block, a zero
/// flag, the password and the account's social name word (zero until a social
/// sign-on has one). A reconnect (the client was in its connection-lost state)
/// instead carries the previous connection's four seeds after the server token
/// and no credentials at all; the server recognises the account from the
/// identity that follows the block.
pub(super) fn push_rsa_stub_with_auth(
    writer: &mut ByteWriter,
    server_token: i64,
    params: &LoginParams,
    seeds: &LoginSeeds,
) -> Result<()> {
    push_rsa_start(writer, server_token, seeds, params.auth.reconnect);
    if params.auth.reconnect {
        return Ok(());
    }
    push_authenticator(writer, &params.auth)?;
    writer.p1(0); // pbool(false)
    writer.pjstr(&params.password)?;
    writer.p8s(params.sso.as_ref().map_or(0, |sso| sso.social_name)); // social name
    writer.p8s(0); // unused 64-bit login field
    Ok(())
}

/// The identity after the RSA block: the username, or, once a social sign-on
/// has been given a key, the flag 0 and that key.
pub(super) fn push_identity(writer: &mut ByteWriter, params: &LoginParams) -> Result<()> {
    match params.sso.as_ref().and_then(|sso| sso.social_key) {
        Some(social_key) => {
            writer.p1(0);
            writer.p8s(social_key);
        }
        None => {
            writer.p1(1);
            writer.pjstr(&params.username)?;
        }
    }
    Ok(())
}

/// Whether the login being built is a game login (the connection to a world)
/// or a lobby login; the two differ in their blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionKind {
    Lobby,
    World,
}

/// The `INIT_SOCIAL_NETWORK_CONNECTION` (29) payload that opens a social
/// sign-on: the game login's header, then an RSA block naming the network.
/// The server answers with the page to sign in on.
pub fn social_connection_payload(
    kind: ConnectionKind,
    params: &LoginParams,
    server_token: i64,
) -> Result<Vec<u8>> {
    Ok(compose_social_connection(kind, params, server_token)?.payload)
}

/// A login body as it goes on the wire (sealed when the login is encrypted)
/// and the session seeds it carries.
pub(super) struct Composed {
    payload: Vec<u8>,
    seeds: Option<SessionSeeds>,
}

/// Seal `plain` when the login is encrypted: `rsa` is the region to encrypt,
/// `tiny_from` where the tiny-cipher region starts.
pub(super) fn finish_body(
    drawn: Option<(&crate::login_crypto::LoginSecurity, LoginSeeds)>,
    plain: Vec<u8>,
    rsa: Option<std::ops::Range<usize>>,
    tiny_from: Option<usize>,
) -> Result<Composed> {
    Ok(match drawn {
        None => Composed {
            payload: plain,
            seeds: None,
        },
        Some((security, seeds)) => Composed {
            payload: security.seal(&plain, rsa, tiny_from, seeds.current)?,
            seeds: Some(seeds.current),
        },
    })
}

pub(super) fn compose_social_connection(
    kind: ConnectionKind,
    params: &LoginParams,
    server_token: i64,
) -> Result<Composed> {
    let sso = params
        .sso
        .as_ref()
        .context("a social sign-on needs its network")?;
    let drawn = params.crypto.begin()?;
    let seeds = drawn.as_ref().map_or(LoginSeeds::ZERO, |(_, seeds)| *seeds);
    let mut writer = ByteWriter::default();
    writer.p4s(910);
    writer.p4s(1);
    if kind == ConnectionKind::World {
        writer.p1(u8::from(params.auth.reconnect));
    }
    let rsa_from = writer.data.len();
    push_rsa_start(&mut writer, server_token, &seeds, params.auth.reconnect);
    push_authenticator(&mut writer, &params.auth)?;
    writer.p1(sso.network as u8);
    writer.p1(params.launcher.language);
    writer.p4s(params.launcher.affiliate);
    for _ in 0..5 {
        writer.p4s(0); // random words
    }
    writer.p8s(0); // unused 64-bit login field
    writer.p1(params.launcher.game);
    writer.p1(0); // random byte
    let rsa_to = writer.data.len();
    finish_body(drawn, writer.data, Some(rsa_from..rsa_to), None)
}

/// Full on-wire `INIT_SOCIAL_NETWORK_CONNECTION` packet.
pub fn build_social_connection_packet(
    kind: ConnectionKind,
    params: &LoginParams,
    server_token: i64,
) -> Result<Vec<u8>> {
    Ok(seal_social_connection_packet(kind, params, server_token)?.0)
}

/// The packet and the seeds it carries (`None` without encryption).
pub(super) fn seal_social_connection_packet(
    kind: ConnectionKind,
    params: &LoginParams,
    server_token: i64,
) -> Result<(Vec<u8>, Option<SessionSeeds>)> {
    let composed = compose_social_connection(kind, params, server_token)?;
    Ok((
        frame_login_packet(
            proto::login::INIT_SOCIAL_NETWORK_CONNECTION,
            composed.payload,
        )?,
        composed.seeds,
    ))
}

/// The client-preferences block: a length byte and the encoded options.
///
/// The lobby parse reads 58 bytes after the length (38 `g1` reads and 20
/// skipped), which is what `ClientOptions::encode` writes; the game-world
/// parse skips as many as the length says.
pub(super) fn push_preferences(writer: &mut ByteWriter, preferences: &[u8]) {
    writer.p1(preferences.len() as u8);
    writer.pdata(preferences);
}

/// The window fields of a login: mode, canvas width and height, anti-aliasing.
pub(super) fn push_window_report(writer: &mut ByteWriter, client: &ClientReport) {
    writer.p1(client.window_mode);
    writer.p2(client.canvas[0]);
    writer.p2(client.canvas[1]);
    writer.p1(client.anti_aliasing);
}

/// The hardware block: `p1(8)`, 38 fixed bytes and 7 `pjstr2` strings.
///
/// An all-unknown machine ([`Hardware::default`]) has empty strings, each
/// costing 2 bytes, so the block is `39 + 7 * 2 = 53` bytes, which the server
/// parse (`Lobby.ts`, `GameLogin.ts`) reads with any string lengths. The
/// runtime-vendor and version bytes describe a managed runtime this client
/// does not have and stay 0; the DirectX driver fields stay empty (DirectX is
/// unavailable by design, as on any non-Windows machine).
pub(super) fn push_hardware_block(writer: &mut ByteWriter, hardware: &Hardware) -> Result<()> {
    let clamp_u16 = |v: u32| u16::try_from(v).unwrap_or(u16::MAX);
    let clamp_u8 = |v: u32| u8::try_from(v).unwrap_or(u8::MAX);
    writer.p1(8); // marker
    writer.p1(hardware.os_id);
    writer.p1(u8::from(hardware.os_64bit));
    writer.p2(hardware.os_version_code);
    writer.p1(0); // runtime vendor
    writer.p1(0); // runtime major
    writer.p1(0); // runtime minor
    writer.p1(0); // runtime patch
    writer.p1(0); // unused
    writer.p2(clamp_u16(hardware.memory_budget_mb));
    writer.p1(clamp_u8(hardware.logical_cpus)); // available processors
    p3(writer, hardware.ram_mb.min(0x00FF_FFFF));
    writer.p2(clamp_u16(hardware.cpu_mhz));
    pjstr2(writer, &hardware.gpu_description)?;
    pjstr2(writer, "")?; // unused
    pjstr2(writer, "")?; // dx driver version
    pjstr2(writer, "")?; // unused
    writer.p1(0); // dx driver month
    writer.p2(0); // dx driver year
    pjstr2(writer, &hardware.cpu_vendor)?;
    pjstr2(writer, &hardware.cpu_description)?;
    writer.p1(clamp_u8(hardware.logical_cpus)); // processors
    writer.p1(hardware.cpu_logical_per_package);
    writer.p4s(hardware.cpu_features[0] as i32);
    writer.p4s(hardware.cpu_features[1] as i32);
    writer.p4s(hardware.cpu_features[2] as i32);
    writer.p4s(hardware.cpu_signature as i32);
    pjstr2(writer, "")?; // unused
    Ok(())
}

/// The 41 archive checksum words (42 archives minus the loading sprites).
/// The server reads exactly 41 `g4`s (`Lobby.ts`).
pub(super) fn push_archive_checksums(
    writer: &mut ByteWriter,
    checksums: &[i32; ARCHIVE_CHECKSUM_WORDS],
) {
    for &checksum in checksums {
        writer.p4s(checksum);
    }
}

/// `LOBBYLOGIN` (19) payload for `params` after the handshake returned
/// `server_token` (lobby login; server side `Lobby.ts` lines 77-226).
pub fn lobby_login_payload(params: &LoginParams, server_token: i64) -> Result<Vec<u8>> {
    Ok(compose_lobby_login(params, server_token)?.payload)
}

pub(super) fn compose_lobby_login(params: &LoginParams, server_token: i64) -> Result<Composed> {
    let drawn = params.crypto.begin()?;
    let seeds = drawn.as_ref().map_or(LoginSeeds::ZERO, |(_, seeds)| *seeds);
    let mut writer = ByteWriter::default();
    writer.p4s(910);
    writer.p4s(1);
    let rsa_from = writer.data.len();
    push_rsa_stub_with_auth(&mut writer, server_token, params, &seeds)?;
    let rsa_to = writer.data.len();
    push_identity(&mut writer, params)?;
    push_lobby_login_tail(&mut writer, params)?;
    finish_body(drawn, writer.data, Some(rsa_from..rsa_to), Some(rsa_to))
}

/// The lobby login from the game mode on: what follows the identity in a
/// `LOBBYLOGIN`, and all of a social network login's block.
pub(super) fn push_lobby_login_tail(writer: &mut ByteWriter, params: &LoginParams) -> Result<()> {
    let launcher = &params.launcher;
    writer.p1(launcher.game);
    writer.p1(launcher.language);
    push_window_report(writer, &params.client);
    push_uid192(writer, &params.uid192);
    writer.pjstr(&params.site_settings)?; // site settings
    push_preferences(writer, &params.client.preferences);
    push_hardware_block(writer, &params.hardware)?;
    writer.p4s(params.verify_id);
    writer.pjstr(&launcher.label)?;
    writer.p4s(launcher.affiliate);
    writer.p4s(launcher.build);
    writer.pjstr(&launcher.gamepack)?;
    writer.p1((launcher.client_type & 1) as u8);
    writer.p1(0); // pbool(false)
    push_archive_checksums(writer, &params.archive_checksums);
    Ok(())
}

/// `GAMELOGIN` (16) payload. The layout is the client's, field for field:
/// ```text
/// p4(910) p4(1) p1(0 = fresh login, 1 = reconnect)
/// | RSA-stub | p1(1) pjstr(username)   or   p1(0) p8(social key)
/// | p1(window mode) p2(canvas width) p2(canvas height) p1(anti-aliasing)
/// | uid192 (24 x -1 if absent) | pjstr(site settings)
/// | p4(affiliate) | p1(length) + the encoded options | hardware block
/// | p4 x 5 (verify counter, userFlow2, userFlow1, two flag words)
/// | pjstr(label) | p1(0 = no additional info, else 1 and pjstr(note))
/// | p1(javascript) p1(chrome) p1(client type & 1)
/// | p4(build) | pjstr(gamepack)
/// | p1(0 = the advertised world, 1 = a switched world)
/// | p2(lobby node = 1) | 41 x p4(archive checksum)
/// ```
pub fn game_login_payload(params: &LoginParams, server_token: i64) -> Result<Vec<u8>> {
    Ok(compose_game_login(params, server_token)?.payload)
}

pub(super) fn compose_game_login(params: &LoginParams, server_token: i64) -> Result<Composed> {
    let drawn = params.crypto.begin()?;
    let seeds = drawn.as_ref().map_or(LoginSeeds::ZERO, |(_, seeds)| *seeds);
    let mut writer = ByteWriter::default();
    writer.p4s(910);
    writer.p4s(1);
    writer.p1(u8::from(params.auth.reconnect)); // 1 on a reconnect
    let rsa_from = writer.data.len();
    push_rsa_stub_with_auth(&mut writer, server_token, params, &seeds)?;
    let rsa_to = writer.data.len();
    push_identity(&mut writer, params)?;
    push_game_login_tail(&mut writer, params)?;
    finish_body(drawn, writer.data, Some(rsa_from..rsa_to), Some(rsa_to))
}

/// The game login from the window mode on: what follows the identity in a
/// `GAMELOGIN`, and all of a social network login's block.
pub(super) fn push_game_login_tail(writer: &mut ByteWriter, params: &LoginParams) -> Result<()> {
    let launcher = &params.launcher;
    push_window_report(writer, &params.client);
    push_uid192(writer, &params.uid192);
    writer.pjstr(&params.site_settings)?; // site settings
    writer.p4s(launcher.affiliate);
    push_preferences(writer, &params.client.preferences);
    push_hardware_block(writer, &params.hardware)?;
    writer.p4s(params.verify_id);
    writer.p4s(launcher.user_flow[0]);
    writer.p4s(launcher.user_flow[1]);
    writer.p4s(launcher.flags[0]);
    writer.p4s(launcher.flags[1]);
    writer.pjstr(&launcher.label)?;
    match &launcher.additional_info {
        Some(info) => {
            writer.p1(1);
            writer.pjstr(info)?;
        }
        None => writer.p1(0),
    }
    writer.p1(u8::from(launcher.javascript));
    writer.p1(u8::from(launcher.chrome));
    writer.p1((launcher.client_type & 1) as u8);
    writer.p4s(launcher.build);
    writer.pjstr(&launcher.gamepack)?;
    writer.p1(u8::from(params.switched_world)); // 0 = the advertised world
    writer.p2(1); // current lobby node
    push_archive_checksums(writer, &params.archive_checksums);
    Ok(())
}

/// `SOCIAL_NETWORK_LOGIN` (30) payload: the login of a sign-on that has just
/// been given its key on this connection. The server already knows the
/// account from the sign-on, so the block has no header, RSA block or
/// identity: it is the login body from its first client setting on.
pub fn social_login_payload(kind: ConnectionKind, params: &LoginParams) -> Result<Vec<u8>> {
    social_login_body(kind, params, None)
}

/// The sign-on login body; with `seeds` all of it is under the tiny cipher
/// keyed by them (the seeds the sign-on's opening block carried).
pub(super) fn social_login_body(
    kind: ConnectionKind,
    params: &LoginParams,
    seeds: Option<SessionSeeds>,
) -> Result<Vec<u8>> {
    let mut writer = ByteWriter::default();
    match kind {
        ConnectionKind::Lobby => push_lobby_login_tail(&mut writer, params)?,
        ConnectionKind::World => push_game_login_tail(&mut writer, params)?,
    }
    let mut body = writer.data;
    if let Some(seeds) = seeds {
        let end = body.len();
        rs910_protocol::tiny_cipher::encrypt_range(&mut body, 0, end, &seeds);
    }
    Ok(body)
}

/// Full on-wire `SOCIAL_NETWORK_LOGIN` packet.
pub fn build_social_login_packet(kind: ConnectionKind, params: &LoginParams) -> Result<Vec<u8>> {
    seal_social_login_packet(kind, params, None)
}

/// The sign-on login packet, under the tiny cipher when `seeds` are given.
pub(super) fn seal_social_login_packet(
    kind: ConnectionKind,
    params: &LoginParams,
    seeds: Option<SessionSeeds>,
) -> Result<Vec<u8>> {
    frame_login_packet(
        proto::login::SOCIAL_NETWORK_LOGIN,
        social_login_body(kind, params, seeds)?,
    )
}

/// Frame a login payload: `p1(opcode) + p2(len) + payload`.
/// `GAMELOGIN` (16) and `LOBBYLOGIN` (19) are both login size `-2`.
pub(super) fn frame_login_packet(opcode: u8, payload: Vec<u8>) -> Result<Vec<u8>> {
    let len = u16::try_from(payload.len())
        .map_err(|_| anyhow::anyhow!("login payload too large: {} bytes", payload.len()))?;
    let mut out = Vec::with_capacity(3 + payload.len());
    out.push(opcode);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(&payload);
    Ok(out)
}

/// Full on-wire `LOBBYLOGIN` packet (opcode + `u16` size + payload).
pub fn build_lobby_login_packet(params: &LoginParams, server_token: i64) -> Result<Vec<u8>> {
    Ok(seal_lobby_login_packet(params, server_token)?.0)
}

/// The packet and the seeds it carries (`None` without encryption).
pub(super) fn seal_lobby_login_packet(
    params: &LoginParams,
    server_token: i64,
) -> Result<(Vec<u8>, Option<SessionSeeds>)> {
    let composed = compose_lobby_login(params, server_token)?;
    Ok((
        frame_login_packet(proto::login::LOBBYLOGIN, composed.payload)?,
        composed.seeds,
    ))
}

/// Full on-wire `GAMELOGIN` packet (opcode + `u16` size + payload).
pub fn build_game_login_packet(params: &LoginParams, server_token: i64) -> Result<Vec<u8>> {
    Ok(seal_game_login_packet(params, server_token)?.0)
}

/// The packet and the seeds it carries (`None` without encryption).
pub(super) fn seal_game_login_packet(
    params: &LoginParams,
    server_token: i64,
) -> Result<(Vec<u8>, Option<SessionSeeds>)> {
    let composed = compose_game_login(params, server_token)?;
    Ok((
        frame_login_packet(proto::login::GAMELOGIN, composed.payload)?,
        composed.seeds,
    ))
}

/// Bare `INIT_GAME_CONNECTION` (14) byte. Login size 0: no length.
#[must_use]
pub fn init_game_connection_packet() -> Vec<u8> {
    vec![proto::login::INIT_GAME_CONNECTION]
}

/// The client-owned values written into `CREATE_ACCOUNT_CONNECT`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CreateConnectInfo {
    /// The uid.dat bytes the login sends (all-zero = absent).
    pub uid192: [u8; 24],
    /// The launcher's parameters; the two user-flow words are the current
    /// ones (`Engine::user_flow`).
    pub launcher: LauncherReport,
    /// The machine the hardware block describes.
    pub hardware: Hardware,
    /// How the block is protected ([`LoginCrypto::Plain`]: not at all).
    pub crypto: LoginCrypto,
}

/// The 24-byte uid block: a uid.dat that is missing, short or all zero is sent
/// as 24 bytes of -1.
pub(super) fn push_uid192(writer: &mut ByteWriter, uid192: &[u8; 24]) {
    if uid192.iter().all(|&byte| byte == 0) {
        writer.pdata(&[0xFF; 24]);
    } else {
        writer.pdata(uid192);
    }
}

/// Build the `CREATE_ACCOUNT_CONNECT` login packet, field for field:
/// ```text
/// p1(28) p2(size) | p2(910) p2(1)
/// | handshake block: p1(10) 4 x p4 seeds 10 x p4 p2        [RSA-encrypted]
/// | pjstr gamepack | p2 affiliate | p4 userFlow2 | p4 userFlow1
/// | pjstr label | p1 language | p1 game | uid192 (24)
/// | p1 additional-info flag [+ pjstr] | hardware block
/// | 7 padding bytes                                        [tiny cipher from
///                                                           the gamepack on]
/// ```
/// The launcher's parameters are written as they are. The block's seeds and
/// filler words are zero without encryption and random words below 99999999
/// with it.
#[cfg(test)]
pub(super) fn create_account_connect_packet(info: &CreateConnectInfo) -> Result<Vec<u8>> {
    Ok(seal_create_account_connect_packet(info)?.0)
}

/// The packet and the seeds it carries (`None` without encryption).
pub(super) fn seal_create_account_connect_packet(
    info: &CreateConnectInfo,
) -> Result<(Vec<u8>, Option<SessionSeeds>)> {
    let launcher = &info.launcher;
    let security = info.crypto.security()?;
    // A filler word: random below 99999999 with encryption, zero without.
    let filler = || security.map_or(0, crate::login_crypto::LoginSecurity::account_word);
    let mut body = ByteWriter::default();
    body.p2(910);
    body.p2(1);
    let rsa_from = body.data.len();
    body.p1(10);
    let seeds: SessionSeeds = security.map_or(
        [0; 4],
        crate::login_crypto::LoginSecurity::draw_account_seeds,
    );
    for seed in seeds {
        body.p4s(seed);
    }
    for _ in 0..10 {
        body.p4s(filler());
    }
    body.p2(filler() as u16);
    let rsa_to = body.data.len();
    body.pjstr(&launcher.gamepack)?;
    body.p2(launcher.affiliate as u16);
    body.p4s(launcher.user_flow[0]);
    body.p4s(launcher.user_flow[1]);
    body.pjstr(&launcher.label)?;
    body.p1(launcher.language);
    body.p1(launcher.game);
    push_uid192(&mut body, &info.uid192);
    match &launcher.additional_info {
        Some(note) => {
            body.p1(1);
            body.pjstr(note)?;
        }
        None => body.p1(0),
    }
    push_hardware_block(&mut body, &info.hardware)?;
    body.pdata(&[0; 7]); // seven bytes of padding, left zero
    let composed = match security {
        None => Composed {
            payload: body.data,
            seeds: None,
        },
        Some(security) => Composed {
            payload: security.seal(&body.data, Some(rsa_from..rsa_to), Some(rsa_to), seeds)?,
            seeds: Some(seeds),
        },
    };
    Ok((
        frame_login_packet(proto::login::CREATE_ACCOUNT_CONNECT, composed.payload)?,
        composed.seeds,
    ))
}

/// Bare `GAMELOGIN_CONTINUE` (26) byte. Login size 0: no length.
#[must_use]
pub fn game_login_continue_packet() -> Vec<u8> {
    vec![proto::login::GAMELOGIN_CONTINUE]
}
