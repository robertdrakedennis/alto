//! The client's JS5 layer: the cache protocol client, its request queues and
//! the resource providers that fetch, verify and prefetch groups.
//!
//! Owners:
//! - [`TcpClient`]: the JS5 socket client (urgent/prefetch queues with the
//!   500-request limits, request framing, chunked reply parsing, stream login
//!   status, close/error handling, stream replacement that re-queues
//!   in-flight requests).
//! - [`HttpClient`]: the `/ms` HTTP content path, used by `AUDIOSTREAMS`.
//! - [`DiskCache`]: the disk thread with its queue over the
//!   [`crate::cache::DiskStore`]s.
//! - [`MasterIndex`], [`Js5Index`]: the master and archive indexes with their
//!   CRC-32 and Whirlpool checks.
//! - [`NetResourceProvider`]: index fetch and verification, urgent/verify/
//!   prefetch group modes, CRC/digest/version checks and retry,
//!   verify-all/prefetch-all, orphan discard, accounting.
//! - [`Js5Client`]: master index, providers, update.
//! - [`Js5`]: the archive readiness/percentage owner.
//! - [`Js5System`]: the JS5 statics (TCP/HTTP/disk clients, providers,
//!   archives), the per-loop processing, error handling, archive creation and
//!   the loadable-resource manager.
//!
//! Disk model: downloaded groups live in `main_file_cache` stores and every
//! group is verified against the server's index. Here the pack directory
//! is the pre-populated half of each store and the client's writable half is
//! the [`crate::cache::DiskOverlay`] (`--cache-dir`); both are read through
//! [`crate::cache::DiskStore`]. The master index always comes from the
//! content server.
//!
//! Wire truth (server counterpart `server/src/lostcity/engine/Lobby.ts`):
//! handshake `p1(15) p1(len) p4(910) p4(1) pjstr(gamepack) p1(lang)`, reply
//! `p1(0)` + one `p4` length per loadable resource; requests
//! `p1(opcode) p5(archive<<32|group)` (opcodes 0 prefetch, 1 urgent, 2/3 login
//! status, 6 new stream, 7 close); replies `p1(archive) p4(group | prefetch
//! bit)` + container bytes, re-headed every 102400 bytes.

#[cfg(test)]
use crate::cache::Stored;
#[cfg(test)]
use std::io::{Read, Write};
#[cfg(test)]
use std::sync::Mutex;

mod requests;
pub use requests::{Request, RequestRef};
mod disk;
pub use disk::{DiskCache, WorkerRequest};
mod tcp;
pub use tcp::{Js5Stream, TcpClient};
mod http;
pub use http::{HttpClient, HttpRequest};
mod indexes;
pub use indexes::{Js5Index, MasterIndex, MasterIndexArchive};
mod provider;
pub use provider::{Net, NetResourceProvider, ProviderSpec};
mod client;
pub use client::Js5Client;
mod archive;
pub use archive::Js5;
mod resources;
use resources::dll;
use resources::library_base_path;
use resources::map_library_name;
use resources::LOADERS;
pub use resources::{LoadableResources, Loader};

use std::collections::{BTreeSet, HashMap};

use std::io::ErrorKind;

use std::net::TcpStream;

use std::path::Path;

use std::sync::Arc;

use std::time::Duration;

use anyhow::Context;

use rs910_core::logic_clock;

use crate::cache::{DiskOverlay, DiskStore, Pack, ARCHIVE_SET};

/// JS5 handshake opcode (`INIT_JS5REMOTE_CONNECTION` of the login opcode table).
pub const OPCODE_JS5_HANDSHAKE: u8 = 15;

/// Urgent group request (plain group id in the reply).
pub const OPCODE_JS5_URGENT: u8 = 1;

/// Prefetch group request (reply sets `group | 0x8000_0000`).
pub const OPCODE_JS5_PREFETCH: u8 = 0;

/// Build major the handshake sends.
pub const BUILD_MAJOR: u32 = 910;

/// Build minor the handshake sends.
pub const BUILD_MINOR: u32 = 1;

/// Reply chunk size including the 5-byte header.
pub const JS5_CHUNK_TOTAL: usize = 102_400;

/// Queue limit of each TCP request queue.
pub const QUEUE_LIMIT: usize = 500;

/// Number of loadable resources the handshake reply lists.
pub const LOADABLE_RESOURCES: usize = 27;

/// Hard cap for a single container allocation (a corrupt header claiming
/// more is a stream error before allocating).
pub const MAX_CONTAINER_BYTES: usize = 64 << 20;

/// Whether the JS5 stream is XOR-obfuscated (off in this revision).
const ENABLE_JS5_XOR: bool = false;

/// Maximum in-flight HTTP requests before new ones are refused.
const HTTP_PENDING_LIMIT: i32 = 10;

/// Encode the `INIT_JS5REMOTE_CONNECTION` (15) handshake bytes.
///
/// Layout parsed by `Lobby.ts:47-50` in this order:
/// ```text
/// p1(15) | p1(payload_len u8) | p4(build_major) | p4(build_minor)
/// | pjstr(token) (NUL-terminated) | p1(lang)
/// ```
/// `payload_len = 4 + 4 + token.len() + 1 + 1`; errors when the token does not
/// fit the `u8` length prefix or falls outside Windows-1252.
pub fn encode_js5_handshake(
    build_major: u32,
    build_minor: u32,
    token: &str,
    lang: u8,
) -> anyhow::Result<Vec<u8>> {
    let mut payload = native910::packet::ByteWriter::default();
    let major = i32::try_from(build_major)
        .map_err(|_| anyhow::anyhow!("build_major {build_major} does not fit i32"))?;
    let minor = i32::try_from(build_minor)
        .map_err(|_| anyhow::anyhow!("build_minor {build_minor} does not fit i32"))?;
    payload.p4s(major);
    payload.p4s(minor);
    payload
        .pjstr(token)
        .with_context(|| format!("handshake token {token:?} outside Windows-1252"))?;
    payload.p1(lang);
    let len = u8::try_from(payload.data.len()).map_err(|_| {
        anyhow::anyhow!(
            "handshake payload {} bytes does not fit u8 length",
            payload.data.len()
        )
    })?;
    let mut out = Vec::with_capacity(2 + payload.data.len());
    out.push(OPCODE_JS5_HANDSHAKE);
    out.push(len);
    out.extend_from_slice(&payload.data);
    Ok(out)
}

/// The group checksum (`getcrc`): table CRC-32,
/// as a signed 32-bit integer. The table CRC is `rs910_core::checksum::crc32` since
/// Phase 2.1 (this module's copy moved there).
#[must_use]
pub fn getcrc(data: &[u8]) -> i32 {
    rs910_core::checksum::crc32(data) as i32
}

/// The name hash: the 31-multiplier string hash of a lowercased name.
#[must_use]
pub fn name_hash(name: &str) -> i32 {
    let mut h = 0_i32;
    for unit in name.to_lowercase().encode_utf16() {
        let b = i32::from(rs910_core::cp1252::cp1252_encode_unit(unit) as i8);
        h = (h << 5).wrapping_sub(h).wrapping_add(b);
    }
    h
}

// ---------------------------------------------------------------------------
// Js5 (the archive)
// ---------------------------------------------------------------------------

/// The archives whose index checksums a login reports, in the order of the
/// archive list: every archive but the loading sprites (32).
pub const LOGIN_CHECKSUM_ARCHIVES: [u32; 41] = [
    0, 1, 2, 3, 5, 7, 8, 10, 12, 13, 14, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29,
    30, 31, 33, 34, 35, 40, 41, 42, 47, 48, 49, 52, 53, 54, 55, 56,
];

// ---------------------------------------------------------------------------
// The Client JS5 statics
// ---------------------------------------------------------------------------

/// The content server address.
#[derive(Clone, Debug)]
pub struct ContentAddress {
    pub host: String,
    pub port: u16,
    pub port2: u16,
    use_secondary: bool,
    use_proxy: bool,
}

impl ContentAddress {
    #[must_use]
    pub fn new(host: String, port: u16, port2: u16) -> Self {
        Self {
            host,
            port,
            port2,
            use_secondary: false,
            use_proxy: false,
        }
    }

    /// Switch to the secondary port, then the proxy.
    fn configure_socket_type(&mut self) {
        if !self.use_secondary {
            self.use_secondary = true;
            self.use_proxy = true;
        } else if self.use_proxy {
            self.use_proxy = false;
        } else {
            self.use_secondary = false;
        }
    }

    /// Opens the socket. The proxy variant reads the host browser's proxy settings; this
    /// port has none, so it connects directly on the chosen port.
    fn socket(&self) -> std::io::Result<TcpStream> {
        use std::net::ToSocketAddrs;
        let port = if self.use_secondary {
            self.port2
        } else {
            self.port
        };
        let addr = (self.host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| std::io::Error::new(ErrorKind::NotFound, "content host"))?;
        TcpStream::connect_timeout(&addr, Duration::from_secs(10))
    }
}

/// The JS5 statics and their per-loop owners.
pub struct Js5System {
    pub tcp: TcpClient,
    pub http: HttpClient,
    pub disk: DiskCache,
    /// The JS5 client, once created.
    pub client: Option<Js5Client>,
    /// The per-archive readiness owners, by archive id.
    archives: HashMap<u32, Js5>,
    /// Archives with a provider (set by `create_js5`).
    js5_providers: BTreeSet<u32>,
    content: ContentAddress,
    gamepack: String,
    language: u8,
    /// Connection state, socket and stream.
    connect_state: i32,
    socket: Option<TcpStream>,
    stream: Option<Js5Stream>,
    /// Reconnect delay, last error count and handshake start.
    backoff: i32,
    last_error_count: i32,
    connect_started: i64,
    pack: Pack,
    master_store: Arc<DiskStore>,
    stores: HashMap<u32, Arc<DiskStore>>,
    pub loadable: LoadableResources,
    /// The registered native-library names; `None` before the platform
    /// loader exists.
    libraries: Option<BTreeSet<String>>,
    /// Groups a `Pack` read found absent, fetched with urgent requests.
    wanted: Vec<(u32, u32)>,
    /// A fatal JS5 error: the client stops (`state = 2`).
    pub fatal: Option<String>,
    logged_out: Option<bool>,
}

impl Js5System {
    /// Startup: the disk cache, the TCP client and the HTTP client of the
    /// HTTP content server.
    #[must_use]
    pub fn new(
        pack_root: &Path,
        overlay: Arc<DiskOverlay>,
        content: ContentAddress,
        http_content: (String, u16),
        game: i32,
        gamepack: String,
        language: u8,
    ) -> Self {
        let pack = Pack::open_with_overlay(pack_root, Some(overlay.clone()));
        let master_store = Arc::new(DiskStore::new(ARCHIVE_SET, pack.clone(), overlay));
        Self {
            tcp: TcpClient::default(),
            http: HttpClient::new(http_content.0, http_content.1, game),
            disk: DiskCache::new(),
            client: None,
            archives: HashMap::new(),
            js5_providers: BTreeSet::new(),
            content,
            gamepack,
            language,
            connect_state: 0,
            socket: None,
            stream: None,
            backoff: 0,
            last_error_count: 0,
            connect_started: 0,
            pack,
            master_store,
            stores: HashMap::new(),
            loadable: LoadableResources::default(),
            libraries: None,
            wanted: Vec::new(),
            fatal: None,
            logged_out: None,
        }
    }

    fn store(&mut self, archive: u32) -> Arc<DiskStore> {
        let pack = self.pack.clone();
        let overlay = self.master_store.overlay().clone();
        self.stores
            .entry(archive)
            .or_insert_with(|| Arc::new(DiskStore::new(archive, pack, overlay)))
            .clone()
    }

    /// One main-loop JS5 step: process the TCP client, then update the JS5
    /// client, plus the login status sent when crossing the logged-in boundary
    /// and the urgent fetches of absent groups.
    pub fn mainloop(&mut self, client_state: i32) {
        let logged_out = client_state == 1
            || rs910_core::client_state::is_title(client_state)
            || rs910_core::client_state::is_lobby(client_state);
        if let Some(previous) = self.logged_out.replace(logged_out) {
            if previous != logged_out {
                self.tcp.send_login_status(!logged_out);
            }
        }
        if !self.tcp.process() {
            self.js5_error(client_state);
        }
        if let Some(client) = self.client.as_mut() {
            let mut net = Net {
                tcp: &mut self.tcp,
                http: &mut self.http,
                disk: &self.disk,
            };
            if let Err(error) = client.update(&mut net) {
                self.fatal.get_or_insert(format!("js5: {error:#}"));
            }
        }
        for missing in self.master_store.overlay().take_missing() {
            if !self.wanted.contains(&missing) {
                self.wanted.push(missing);
            }
        }
        if !self.wanted.is_empty() {
            self.ensure_demand_client();
            let wanted = std::mem::take(&mut self.wanted);
            for (archive, group) in wanted {
                match self.demand_group(archive, group) {
                    Some(true) => log::debug!("[client910] js5 fetched {archive}/{group}"),
                    Some(false) => self.wanted.push((archive, group)),
                    None if self.master_loaded() => {}
                    None => self.wanted.push((archive, group)),
                }
            }
        }
    }

    /// Handle a JS5 connection error: back off, reconnect and escalate.
    fn js5_error(&mut self, client_state: i32) {
        if self.tcp.error_count > self.last_error_count {
            self.content.configure_socket_type();
            self.backoff = (self.tcp.error_count * 250 - 250).min(3000);
            let (count, state) = (self.tcp.error_count, self.tcp.js5_state);
            let fatal = if count >= 2 && state == 6 {
                Some("js5connect_outofdate".to_owned())
            } else if count >= 1 && state == 48 {
                Some("sessionexpired".to_owned())
            } else if count >= 4 && state == -1 {
                Some(format!(
                    "js5crc a={}&g={}",
                    self.tcp.archive, self.tcp.group
                ))
            } else if count >= 4 && rs910_core::client_state::is_loading(client_state) {
                Some(if state == 7 || state == 9 {
                    "js5connect_full".to_owned()
                } else if state <= 0 {
                    "js5io".to_owned()
                } else {
                    "js5connect".to_owned()
                })
            } else {
                None
            };
            if let Some(code) = fatal {
                self.fatal = Some(code);
                return;
            }
        }
        self.last_error_count = self.tcp.error_count;
        if self.backoff > 0 {
            self.backoff -= 1;
            return;
        }
        if let Err(error) = self.connect_step(client_state) {
            let _ = error;
            self.set_js5_error_state(1002);
        }
    }

    fn connect_step(&mut self, client_state: i32) -> std::io::Result<()> {
        if self.connect_state == 0 {
            self.socket = Some(self.content.socket()?);
            self.connect_state += 1;
        }
        if self.connect_state == 1 {
            let socket = self.socket.take().expect("js5 socket");
            let mut stream = Js5Stream::new(socket, 131_072)?;
            let handshake =
                encode_js5_handshake(BUILD_MAJOR, BUILD_MINOR, &self.gamepack, self.language)
                    .map_err(|e| std::io::Error::new(ErrorKind::InvalidInput, e.to_string()))?;
            stream.write(&handshake)?;
            self.stream = Some(stream);
            self.connect_state += 1;
            self.connect_started = logic_clock::monotonic_millis();
        }
        if self.connect_state == 2 {
            let stream = self.stream.as_mut().expect("js5 stream");
            if stream.has_available(1)? {
                let mut status = [0_u8; 1];
                let read = stream.read(&mut status);
                if status[0] != 0 {
                    // The read count is passed on, not the status byte.
                    self.set_js5_error_state(read as i32);
                    return Ok(());
                }
                self.connect_state += 1;
            } else if logic_clock::monotonic_millis() - self.connect_started > 30000 {
                self.set_js5_error_state(1001);
                return Ok(());
            }
        }
        if self.connect_state == 3 {
            let need = LOADABLE_RESOURCES * 4;
            let stream = self.stream.as_mut().expect("js5 stream");
            if stream.has_available(need)? {
                let mut lengths = vec![0_u8; need];
                stream.read(&mut lengths);
                for (slot, chunk) in self
                    .loadable
                    .lengths
                    .iter_mut()
                    .zip(lengths.chunks_exact(4))
                {
                    *slot = i32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                }
                let logged_out = rs910_core::client_state::is_loading(client_state)
                    || rs910_core::client_state::is_title(client_state)
                    || rs910_core::client_state::is_lobby(client_state);
                let stream = self.stream.take().expect("js5 stream");
                self.tcp.create_new_stream(stream, !logged_out);
                self.socket = None;
                self.connect_state = 0;
                log::info!(
                    "[client910] js5 stream open to {}:{} (errors {})",
                    self.content.host,
                    self.content.port,
                    self.tcp.error_count
                );
            }
        }
        Ok(())
    }

    /// Drop the connection and record the error state.
    fn set_js5_error_state(&mut self, state: i32) {
        self.socket = None;
        self.stream = None;
        self.connect_state = 0;
        self.tcp.error_count += 1;
        self.tcp.js5_state = state;
    }

    /// `Loading` stage 0: `new Js5Client` once.
    pub fn ensure_client(&mut self) {
        if self.client.is_none() {
            let mut net = Net {
                tcp: &mut self.tcp,
                http: &mut self.http,
                disk: &self.disk,
            };
            self.client = Some(Js5Client::new(&mut net));
        }
    }

    /// Advance the master index load; true once it is loaded.
    pub fn load_master_index(&mut self) -> bool {
        let Some(client) = self.client.as_mut() else {
            return false;
        };
        let mut net = Net {
            tcp: &mut self.tcp,
            http: &mut self.http,
            disk: &self.disk,
        };
        match client.load_master_index(&mut net) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.fatal.get_or_insert(format!("js5: {error:#}"));
                false
            }
        }
    }

    pub fn master_loaded(&self) -> bool {
        self.client
            .as_ref()
            .is_some_and(|c| c.master_index.is_some())
    }

    /// Reload: drop the JS5 client and reset the loadable resources.
    pub fn reload(&mut self) {
        self.client = None;
        self.loadable.reset();
    }

    /// Create an archive's provider and readiness owner; only
    /// `AUDIOSTREAMS` fetches over HTTP.
    pub fn create_js5(
        &mut self,
        archive: u32,
        discard_packed: bool,
        discard_unpacked: i32,
        prefetch_all: bool,
    ) -> anyhow::Result<()> {
        let datafs = self.store(archive);
        let masterfs = self.master_store.clone();
        let client = self
            .client
            .as_mut()
            .context("js5: create_js5 without a JS5 client")?;
        let mut net = Net {
            tcp: &mut self.tcp,
            http: &mut self.http,
            disk: &self.disk,
        };
        // Only AUDIOSTREAMS fetches over HTTP.
        let use_http = archive == 40;
        let provider = client.archive_provider(
            &mut net,
            archive,
            Some(datafs),
            Some(masterfs),
            true,
            use_http,
        )?;
        if prefetch_all {
            provider.request_prefetch_all();
        }
        self.js5_providers.insert(archive);
        self.archives
            .insert(archive, Js5::new(archive, discard_packed, discard_unpacked));
        Ok(())
    }

    /// The JS5 connection state (`js5ConnectState`).
    #[must_use]
    pub fn connect_state(&self) -> i32 {
        self.connect_state
    }

    /// The checksum of each archive index a login reports, zero for an archive
    /// that is not loaded ([`LOGIN_CHECKSUM_ARCHIVES`] order).
    pub fn archive_checksums(&mut self) -> [i32; 41] {
        LOGIN_CHECKSUM_ARCHIVES.map(|archive| {
            self.with_archive(archive, |j, p, n| j.checksum(p, n))
                .flatten()
                .unwrap_or(0)
        })
    }

    pub fn remove_archive(&mut self, archive: u32) {
        self.archives.remove(&archive);
    }

    /// Run `f` over one archive's `Js5`, its provider and the net owners.
    fn with_archive<R>(
        &mut self,
        archive: u32,
        f: impl FnOnce(&mut Js5, &mut NetResourceProvider, &mut Net) -> R,
    ) -> Option<R> {
        let js5 = self.archives.get_mut(&archive)?;
        let provider = self.client.as_mut()?.provider_mut(archive)?;
        let mut net = Net {
            tcp: &mut self.tcp,
            http: &mut self.http,
            disk: &self.disk,
        };
        Some(f(js5, provider, &mut net))
    }

    fn with_provider<R>(
        &mut self,
        archive: u32,
        f: impl FnOnce(&mut NetResourceProvider, &mut Net) -> R,
    ) -> Option<R> {
        let provider = self.client.as_mut()?.provider_mut(archive)?;
        let mut net = Net {
            tcp: &mut self.tcp,
            http: &mut self.http,
            disk: &self.disk,
        };
        Some(f(provider, &mut net))
    }

    /// Fetch every group of an archive.
    pub fn fetch_all(&mut self, archive: u32) -> bool {
        self.with_archive(archive, |j, p, n| j.fetch_all(p, n))
            .unwrap_or(false)
    }

    /// Download progress of an archive, in percent.
    pub fn percentage(&mut self, archive: u32) -> i32 {
        self.with_archive(archive, |j, p, n| j.percentage(p, n))
            .unwrap_or(0)
    }

    /// Whether a group of an archive is downloaded.
    pub fn is_group_ready(&mut self, archive: u32, group: i32) -> bool {
        self.with_archive(archive, |j, p, n| j.is_group_ready(p, n, group))
            .unwrap_or(false)
    }

    /// Download progress of a group of an archive, in percent.
    pub fn group_percentage(&mut self, archive: u32, group: i32) -> i32 {
        self.with_archive(archive, |j, p, n| j.group_percentage(p, n, group))
            .unwrap_or(0)
    }

    /// Request a file's group of an archive.
    pub fn request_download(&mut self, archive: u32, group: i32, file: i32) -> bool {
        self.with_archive(archive, |j, p, n| j.request_download(p, n, group, file))
            .unwrap_or(false)
    }

    /// Load a file of an archive by its flat id.
    pub fn load_file(&mut self, archive: u32, id: i32) -> bool {
        self.with_archive(archive, |j, p, n| j.load_file(p, n, id))
            .unwrap_or(false)
    }

    /// Drop every queued sprite id whose file is already loadable from the
    /// sprites archive.
    pub fn drain_sprite_prefetch(&mut self, sprites: &mut Vec<i32>) {
        sprites.retain(|&id| !self.load_file(8, id));
    }

    #[allow(dead_code)]
    pub fn is_file_ready(&mut self, archive: u32, id: i32) -> bool {
        self.with_archive(archive, |j, p, n| j.is_file_ready(p, n, id))
            .unwrap_or(false)
    }

    /// Queue a group of an archive for prefetching.
    #[allow(dead_code)]
    pub fn prefetch_group(&mut self, archive: u32, group: i32) {
        self.with_provider(archive, |p, _| Js5::prefetch_group(p, group));
    }

    /// The index download progress of an archive's provider, in percent.
    pub fn provider_percentage(&mut self, archive: u32) -> i32 {
        self.with_provider(archive, |p, n| p.index_percentage(n))
            .unwrap_or(0)
    }

    /// `GET_JS5_INDEXES`: the average index
    /// percentage over the archives with providers.
    pub fn index_average(&mut self) -> i32 {
        let archives: Vec<u32> = self.js5_providers.iter().copied().collect();
        let mut sum = 0;
        let mut count = 0;
        for archive in archives {
            if let Some(p) = self.with_provider(archive, |p, n| p.index_percentage(n)) {
                sum += p;
                count += 1;
            }
        }
        if count > 0 {
            sum / count
        } else {
            sum
        }
    }

    fn providers_with_prefetch(&self) -> impl Iterator<Item = &NetResourceProvider> {
        self.js5_providers.iter().filter_map(move |&a| {
            self.client
                .as_ref()?
                .providers
                .as_ref()?
                .get(a as usize)?
                .as_ref()
                .filter(|p| p.prefetch_requested())
        })
    }

    /// The `preload_percent`/`preload_progress` inputs; `None` while no
    /// providers exist (the scripts then fail with a null pointer).
    #[must_use]
    pub fn preload_percent(&self) -> Option<i32> {
        if self.js5_providers.is_empty() {
            return None;
        }
        let (mut size, mut loaded) = (0_i32, 0_i32);
        for p in self.providers_with_prefetch() {
            size = size.wrapping_add(p.index_size());
            loaded = loaded.wrapping_add(p.loaded_groups());
        }
        Some(if size == 0 {
            0
        } else {
            loaded.wrapping_mul(100) / size
        })
    }

    /// The debug overlay's cache line inputs:
    /// `[index size, verified, loaded]` over prefetch-requested providers.
    #[must_use]
    pub fn cache_stats(&self) -> [i32; 3] {
        let mut out = [0_i32; 3];
        for p in self.providers_with_prefetch() {
            out[0] = out[0].wrapping_add(p.index_size());
            out[1] = out[1].wrapping_add(p.verified_groups());
            out[2] = out[2].wrapping_add(p.loaded_groups());
        }
        out
    }

    /// Create the native-library platform loader.
    pub fn create_hardware_platform_loader(&mut self) {
        self.libraries.get_or_insert_with(BTreeSet::new);
    }

    /// The download progress of a native library's `dlls` group. `Err(code)`
    /// is a loader failure.
    fn fetch_library(&mut self, name: &str, raw: bool) -> Result<i32, i32> {
        if self.libraries.as_ref().is_some_and(|l| l.contains(name)) {
            return Ok(100);
        }
        let group = if raw {
            name.to_owned()
        } else {
            let file = map_library_name(name).ok_or(1)?;
            let group = format!("{}{file}", library_base_path());
            let valid = self
                .with_archive(30, |j, p, n| j.is_file_name_valid(p, n, &group, ""))
                .unwrap_or(false);
            if !valid {
                return Err(2);
            }
            group
        };
        let ready = self
            .with_archive(30, |j, p, n| j.is_group_ready_named(p, n, &group))
            .unwrap_or(false);
        if !ready {
            return Ok(self
                .with_archive(30, |j, p, n| j.group_percentage_named(p, n, &group))
                .unwrap_or(0));
        }
        // TODO(#native-library-files): the reference writes the group's bytes
        // to the library file and registers it for loading; this port links jaclib/jaggl/sw3d statically, so
        // only the download is performed.
        if let Some(libraries) = self.libraries.as_mut() {
            libraries.insert(name.to_owned());
        }
        Ok(100)
    }

    fn loader_percentage(&mut self, loader: &mut Loader) -> i32 {
        match loader {
            Loader::Archive(archive) => {
                let archive = *archive;
                if self.fetch_all(archive) {
                    100
                } else {
                    self.percentage(archive)
                }
            }
            Loader::File(archive, name) => {
                let (archive, name) = (*archive, *name);
                let ready = self
                    .with_archive(archive, |j, p, n| j.request_group(p, n, name))
                    .unwrap_or(false);
                if ready {
                    100
                } else {
                    0
                }
            }
            Loader::Group(archive, group) => {
                let (archive, group) = (*archive, *group);
                if self.is_group_ready(archive, group) {
                    100
                } else {
                    self.group_percentage(archive, group)
                }
            }
            Loader::Dll { name, raw, failed } => {
                if *failed {
                    return 100;
                }
                match self.fetch_library(name, *raw) {
                    Ok(percent) => percent,
                    Err(_) => {
                        *failed = true;
                        100
                    }
                }
            }
        }
    }

    /// The load progress of the loadable resources. `reloading` is set while
    /// a JS5 reload is in progress.
    pub fn load_progress(&mut self, reloading: bool) -> i32 {
        if self.loadable.state == 0 {
            let mut jaclib = dll("jaclib", false);
            if self.loader_percentage(&mut jaclib) != 100 {
                return 1;
            }
            // The native libraries and the ping helper are linked statically
            // in this port.
            let _ = reloading;
            self.loadable.state = 1;
        }
        if self.loadable.state == 1 {
            let mut loaders = LOADERS.to_vec();
            if !std::env::consts::OS.starts_with("win") {
                // The Direct3D library exists on Windows only.
                if let Loader::Dll { failed, .. } = &mut loaders[3] {
                    *failed = true;
                }
            }
            let mut base = 0;
            for (i, loader) in loaders.iter_mut().enumerate() {
                let length = self.loadable.lengths[i];
                base += length * self.loader_percentage(loader) / 100;
            }
            self.loadable.base = base;
            self.loadable.resources = Some(loaders);
            self.loadable.state = 2;
        }
        let Some(mut loaders) = self.loadable.resources.take() else {
            return 100;
        };
        let (mut total, mut done, mut all) = (0_i32, 0_i32, true);
        for (i, loader) in loaders.iter_mut().enumerate() {
            let length = self.loadable.lengths[i];
            let percent = self.loader_percentage(loader);
            if percent < 100 {
                all = false;
            }
            total += length;
            done += length * percent / 100;
        }
        if !all {
            self.loadable.resources = Some(loaders);
        }
        let done = done - self.loadable.base;
        let total = total - self.loadable.base;
        let mut percent = if total > 0 { done * 100 / total } else { 100 };
        if !all && percent > 99 {
            percent = 99;
        }
        percent
    }

    /// The landscape progress input of a scene rebuild over the map squares:
    /// groups of the maps archive not yet ready, or `None`
    /// before the maps archive and its index exist.
    pub fn map_groups_missing(&mut self, groups: &[u16]) -> Option<usize> {
        const MAPS: u32 = 5;
        if !self.ensure_demand_archive(MAPS) {
            return None;
        }
        // A group is never ready while the index is still loading;
        // a square without a group in the loaded
        // index has no map data to wait for.
        let index_ready = self
            .with_archive(MAPS, |j, p, n| j.is_index_ready(p, n))
            .unwrap_or(false);
        if !index_ready {
            return None;
        }
        let mut missing = 0;
        for &group in groups {
            let group = i32::from(group);
            let valid = self
                .with_archive(MAPS, |j, p, n| j.is_group_valid(p, n, group))
                .unwrap_or(false);
            if valid && !self.is_group_ready(MAPS, group) {
                missing += 1;
            }
        }
        Some(missing)
    }

    /// Without the loading stages (`--direct-login`) the owners a demand
    /// fetch needs: `Js5Client` and its master index.
    fn ensure_demand_client(&mut self) {
        self.ensure_client();
        if !self.master_loaded() {
            self.load_master_index();
        }
    }

    /// The archive's `Js5`, created on demand (without prefetch-all) for a
    /// group a consumer read before the loading stages created it.
    fn ensure_demand_archive(&mut self, archive: u32) -> bool {
        if self.archives.contains_key(&archive) {
            return true;
        }
        self.ensure_demand_client();
        if !self.master_loaded() {
            return false;
        }
        let existing = self
            .client
            .as_mut()
            .and_then(|c| c.provider_mut(archive))
            .is_some();
        if existing {
            self.archives.insert(archive, Js5::new(archive, false, 1));
            return true;
        }
        let in_master = self
            .client
            .as_ref()
            .and_then(|c| c.master_index.as_ref())
            .is_some_and(|m| (archive as usize) < m.archives.len());
        in_master && self.create_js5(archive, false, 1, false).is_ok()
    }

    /// `Js5.isGroupReady` of an absent group: `Some(true)` once fetched,
    /// `Some(false)` while pending, `None` when the group is not in the
    /// archive (or the archive cannot exist yet).
    fn demand_group(&mut self, archive: u32, group: u32) -> Option<bool> {
        if !self.ensure_demand_archive(archive) {
            return Some(false).filter(|_| !self.master_loaded());
        }
        let group = i32::try_from(group).ok()?;
        let valid = self.with_archive(archive, |j, p, n| {
            (j.is_index_ready(p, n), j.is_group_valid(p, n, group))
        })?;
        match valid {
            (false, _) => Some(false),
            (true, false) => None,
            (true, true) => Some(self.is_group_ready(archive, group)),
        }
    }

    /// Shut the JS5 layer down: close the connections and stop the disk thread.
    pub fn quit(&mut self) {
        self.tcp.close_gracefully();
        self.http.shutdown();
        self.disk.quit();
    }

    /// Content bytes received (diagnostics).
    #[must_use]
    pub fn bytes_in(&self) -> u64 {
        self.tcp.bytes_in
    }

    /// Outstanding urgent + prefetch requests (diagnostics).
    #[must_use]
    pub fn outstanding(&self) -> (usize, usize) {
        (self.tcp.total_urgents(), self.tcp.total_prefetches())
    }
}

#[cfg(test)]
mod tests;
