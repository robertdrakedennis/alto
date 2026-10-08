//! `CLIENT910_RECORD=<file>`: record one live world session for the headless
//! session replay (`app::session_replay`, fixture
//! `fixtures/session-replay/*.rtr`).
//!
//! The file is the "lite" subset of the RTR1 trace format
//! (`docs/architecture.md`): magic `RTR1`, a
//! little-endian `u32` format version, then records until end of file:
//!
//! ```text
//! i32 cycle | [u8; 4] tag | u32 length | length bytes
//! ```
//!
//! `cycle` is the client's logic cycle (`-1` before the event loop). Tags:
//!
//! | tag    | payload |
//! |--------|---------|
//! | `HEAD` | UTF-8 `key=value` lines: world login reply fields, CLI args |
//! | `SVRC` | server-permanent varcs from the login reply |
//! | `PREF`/`VARC` | preferences / client-variable file bytes before install |
//! | `ENV ` | one `CLIENT910_*` input-injector variable, `NAME=VALUE` |
//! | `INIT` / `IOUT` | world socket bytes read / written by the startup drain |
//! | `TOOL` | renderer AA/bloom support, scene-sample query and answer, window
//! |        | physical size and scale factor (`install_session_toolkit`) |
//! | `CANV` | canvas `w`,`h` (u32), `n` fullscreen modes (4 x i32) |
//! | `GLTF` | the renderer's GL compressed-texture formats (i32 each) |
//! | `NOWM` | `monotonic_millis()` at the start of the cycle (i64) |
//! | `IN  ` / `OUT ` | world socket bytes read / written by the live poll |
//! | `PNGC` | ping-worker result consumed by the logic owner (round trip, i32) |
//! | `LOUT` | `LoadingConnection` keepalive bytes (1 s wall-clock cadence) |
//! | `MAPI` | the map transaction was acknowledged (`MAP_BUILD_COMPLETE`) |
//! | `FOCS` | window focus changed (u8) |
//! | `RSIZ` | a native resize (physical `w`,`h`, u32) |
//! | `CURS` | `ui_cursor::State::current` changed (i32) |
//! | `INPT` | discarded OS input kind in a scripted fixture recording |
//! | `UIEV` | accepted canonical retained input as typed UTF-8 JSON, including
//! |        | its event timestamp; replayed before the following logic cycle |
//!
//! Recording is behaviour-neutral: every hook is a no-op unless the variable
//! is set, and none of them changes the bytes or their order.
use std::io::Write;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Mutex;

pub const MAGIC: &[u8; 4] = b"RTR1";
/// RTR1 "lite" (session replay subset).
pub const VERSION: u32 = 0x0001_0000;

static ACTIVE: AtomicBool = AtomicBool::new(false);
static CYCLE: AtomicI32 = AtomicI32::new(-1);
static WRITER: Mutex<Option<std::io::BufWriter<std::fs::File>>> = Mutex::new(None);

/// Start recording to `CLIENT910_RECORD` (no-op when unset or already on).
pub fn start_from_flags() {
    let Some(path) = crate::client_debug_flags::flags().record.as_ref() else {
        return;
    };
    let mut writer = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if writer.is_some() {
        return;
    }
    match std::fs::File::create(path) {
        Ok(file) => {
            let mut out = std::io::BufWriter::new(file);
            let _ = out.write_all(MAGIC);
            let _ = out.write_all(&VERSION.to_le_bytes());
            let _ = out.flush();
            *writer = Some(out);
            ACTIVE.store(true, Ordering::Release);
            log::info!("[client910] recording session to {}", path.display());
        }
        Err(error) => log::warn!("[client910] CLIENT910_RECORD {}: {error}", path.display()),
    }
}

#[inline]
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Append one record at the current cycle.
pub fn record(tag: &[u8; 4], bytes: &[u8]) {
    if !active() {
        return;
    }
    let mut writer = WRITER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(out) = writer.as_mut() {
        let _ = out.write_all(&CYCLE.load(Ordering::Acquire).to_le_bytes());
        let _ = out.write_all(tag);
        let _ = out.write_all(&(bytes.len() as u32).to_le_bytes());
        let _ = out.write_all(bytes);
        // The client is stopped by killing it; every record must reach the file.
        let _ = out.flush();
    }
}

/// A new logic cycle begins (`ViewerApp::about_to_wait`, after `logic_cycle++`).
pub fn cycle(cycle: i32, now_ms: i64) {
    if !active() {
        return;
    }
    CYCLE.store(cycle, Ordering::Release);
    record(b"NOWM", &now_ms.to_le_bytes());
}

pub fn canvas([w, h]: [u32; 2], modes: &[crate::ui_runtime::FullscreenMode]) {
    if !active() {
        return;
    }
    let mut bytes = Vec::new();
    bytes.extend(w.to_le_bytes());
    bytes.extend(h.to_le_bytes());
    bytes.extend((modes.len() as u32).to_le_bytes());
    for mode in modes {
        for v in [mode.width, mode.height, mode.bit_depth, mode.refresh] {
            bytes.extend(v.to_le_bytes());
        }
    }
    record(b"CANV", &bytes);
}

/// `ui_cursor::State::current` as the logic update reads it (recorded on change).
pub fn cursor(value: i32) {
    static LAST: AtomicI32 = AtomicI32::new(i32::MIN);
    if active() && LAST.swap(value, Ordering::AcqRel) != value {
        record(b"CURS", &value.to_le_bytes());
    }
}

/// The renderer capabilities `install_session_toolkit` read, the scene
/// sample count it asked for and the window's physical size / scale.
pub fn toolkit(
    caps: crate::toolkit_caps::ToolkitCaps,
    samples: u32,
    supported: bool,
    physical: [u32; 2],
    scale: f64,
) {
    if !active() {
        return;
    }
    let mut bytes = vec![
        u8::from(caps.antialiasing),
        u8::from(caps.bloom),
        u8::from(supported),
    ];
    bytes.extend(samples.to_le_bytes());
    bytes.extend(physical[0].to_le_bytes());
    bytes.extend(physical[1].to_le_bytes());
    bytes.extend(scale.to_le_bytes());
    record(b"TOOL", &bytes);
}

pub fn gl_formats(formats: &[i32]) {
    if !active() {
        return;
    }
    let bytes: Vec<u8> = formats.iter().flat_map(|v| v.to_le_bytes()).collect();
    record(b"GLTF", &bytes);
}

/// `AsyncRead + AsyncWrite` pass-through that records the startup drain's
/// world socket bytes as `INIT` / `IOUT`.
pub struct RecordedStream<'a, S>(pub &'a mut S);

impl<S: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for RecordedStream<'_, S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = std::pin::Pin::new(&mut *self.0).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(())) = &poll {
            record(b"INIT", &buf.filled()[before..]);
        }
        poll
    }
}

impl<S: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for RecordedStream<'_, S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let poll = std::pin::Pin::new(&mut *self.0).poll_write(cx, buf);
        if let std::task::Poll::Ready(Ok(n)) = &poll {
            record(b"IOUT", &buf[..*n]);
        }
        poll
    }
    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.0).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut *self.0).poll_shutdown(cx)
    }
}

/// One decoded record.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub cycle: i32,
    pub tag: [u8; 4],
    pub bytes: Vec<u8>,
}

/// Decode a recorded session (a truncated final record is dropped: the
/// recorder is stopped by killing the client).
#[cfg(any(test, feature = "test-hooks"))]
pub fn read(bytes: &[u8]) -> anyhow::Result<Vec<Record>> {
    anyhow::ensure!(
        bytes.len() >= 8 && &bytes[..4] == MAGIC,
        "not an RTR1 trace"
    );
    let version = u32::from_le_bytes(bytes[4..8].try_into()?);
    anyhow::ensure!(version == VERSION, "RTR1 version {version:#x}");
    let mut records = Vec::new();
    let mut pos = 8;
    while pos + 12 <= bytes.len() {
        let cycle = i32::from_le_bytes(bytes[pos..pos + 4].try_into()?);
        let tag: [u8; 4] = bytes[pos + 4..pos + 8].try_into()?;
        let len = u32::from_le_bytes(bytes[pos + 8..pos + 12].try_into()?) as usize;
        if pos + 12 + len > bytes.len() {
            break;
        }
        records.push(Record {
            cycle,
            tag,
            bytes: bytes[pos + 12..pos + 12 + len].to_vec(),
        });
        pos += 12 + len;
    }
    Ok(records)
}
