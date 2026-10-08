//! The client core's way out (target-architecture.md §2.4 #4, code-quality
//! programme Phase 5): what a logic cycle reads from or writes to the world
//! outside the core goes through an [`Io`], so a session can be recorded
//! ([`RecordingIo`], the RTR1 writer behind `CLIENT910_RECORD`) and replayed
//! headless ([`ReplayIo`]).
//!
//! | part | calls | live ([`LiveIo`]) | recording ([`RecordingIo`], RTR1 tags) | replay ([`ReplayIo`]) |
//! |---|---|---|---|---|
//! | clock | [`Io::begin_frame`] (before P0), [`Io::begin_cycle`] (P3, after the cycle counter increments) | the shell's nano timer; P3 samples the monotonic clock as the recorder's call did | `NOWM` at P3 | installs the cycle's recorded `NOWM` (else the last time + 20 ms) as the logic clock from the frame's start |
//! | sockets | [`Io::flush`], [`Io::read`] (P7 lobby read, P8 world read) | `session::{flush_nonblocking, read_nonblocking}` on the connection's socket | world `OUT ` / `IN  ` | the same transport functions over the recorded bytes (no socket) |
//! | platform | [`Io::cursor`] (P9), [`Io::window_event`], [`Io::drops_os_input`] | pass-through | `CURS` on change; `FOCS`, `RSIZ`, `INPT`/`UIEV`; scripted recordings isolate input | the recorded cursor; [`ReplayIo::window_events`] after each cycle |
//!
//! The logic clock itself stays `rs910_core::logic_clock` (every logic read
//! goes through it, Phase 0.3); the `Io` decides what it reads at each cycle
//! boundary. Not routed here (they happen outside the logic cycle or have
//! no replay consumer yet): the startup records (`HEAD`, `SVRC`,
//! `PREF`/`VARC`, `ENV `, `INIT`/`IOUT`, `TOOL`/`CANV`/`GLTF`, `MAPI`, the
//! keep-alive thread's `LOUT`), which `session_record` writes directly; the
//! login/prefetch/host-resolution workers, whose completions the session
//! polls on its own channels (a replay stops at a mid-session map
//! transaction, as before); and the system clipboard (rs910-ui's
//! `clipboard`).
use super::input_event::{InputEvent, RECORD_TAG};
use crate::wire_stream::WireStream;
use std::net::TcpStream;

/// A session connection (the world or the lobby).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conn {
    /// The game (world) connection.
    World,
    /// The lobby connection (also the title account-creation socket).
    Lobby,
}

/// A window event the logic sees, as the recorder writes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowRecord {
    /// A native resize, physical pixels (`RSIZ`).
    Resized([u32; 2]),
    /// Focus gained or lost (`FOCS`).
    Focused(bool),
    /// An operating-system input event (1 key, 2 button, 3 cursor, 4 wheel;
    /// `INPT`): a recorded session is described by its injectors alone.
    Input(u8),
    /// Canonical input accepted by an interactive session.
    AcceptedInput(InputEvent),
}

/// The core's clock, transport and platform boundary (see the module docs).
pub trait Io {
    /// F0/P0: a frame whose first logic cycle will be `next_cycle` begins
    /// (the shell's timer granted cycles; before P0's resize).
    fn begin_frame(&mut self, _next_cycle: i32) {}
    /// P3: logic cycle `cycle` begins (after the cycle counter increments).
    fn begin_cycle(&mut self, cycle: i32);
    /// Queue-flush half of the non-blocking transport
    /// (`session::flush_nonblocking`): writes `pending` to `conn`, handing
    /// each written chunk to `wrote` before it leaves `pending`.
    fn flush(
        &mut self,
        conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        wrote: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<()>;
    /// Read half (`session::read_nonblocking`): appends what `conn` has
    /// buffered to `pending`, each chunk through `read` first; `Ok(true)`
    /// when the peer closed the stream.
    fn read(
        &mut self,
        conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        read: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<bool>;
    /// P9: the cursor the logic update reads (`ui_cursor::State::current`,
    /// the shell's `current`).
    fn cursor(&mut self, current: i32) -> i32;
    /// A window event the shell received.
    fn window_event(&mut self, event: WindowRecord);
    /// True while a recording drops operating-system input (the session is
    /// described by its `CLIENT910_*` injectors alone).
    fn drops_os_input(&self) -> bool {
        false
    }
}

/// Today's behaviour: the operating-system clock (through `logic_clock`),
/// the connections' real sockets and the window's own events.
#[derive(Clone, Copy, Debug, Default)]
pub struct LiveIo;

impl Io for LiveIo {
    fn begin_cycle(&mut self, _cycle: i32) {
        // The recorder's `NOWM` argument was evaluated on every cycle:
        // The monotonic clock's backwards-jump compensation keeps its
        // sample points whether or not a session is recorded.
        let _ = crate::logic_clock::monotonic_millis();
    }
    fn flush(
        &mut self,
        _conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        wrote: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<()> {
        crate::session::flush_nonblocking(stream, pending, wrote)
    }
    fn read(
        &mut self,
        _conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        read: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<bool> {
        crate::session::read_nonblocking(stream, pending, read)
    }
    fn cursor(&mut self, current: i32) -> i32 {
        current
    }
    fn window_event(&mut self, _event: WindowRecord) {}
}

/// `CLIENT910_RECORD`: `inner`'s behaviour plus the RTR1 records of the
/// cycle clock, the world connection's bytes, the cursor and the window
/// events (`session_record`, which writes nothing unless a recording was
/// started).
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordingIo<I = LiveIo> {
    pub inner: I,
    interactive: bool,
}

impl<I> RecordingIo<I> {
    pub fn new(inner: I) -> Self {
        Self {
            inner,
            interactive: false,
        }
    }

    pub fn interactive(inner: I) -> Self {
        Self {
            inner,
            interactive: true,
        }
    }
}

impl<I: Io> Io for RecordingIo<I> {
    fn begin_cycle(&mut self, cycle: i32) {
        crate::session_record::cycle(cycle, crate::logic_clock::monotonic_millis());
    }
    fn flush(
        &mut self,
        conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        wrote: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<()> {
        self.inner.flush(conn, stream, pending, &mut |written| {
            if conn == Conn::World {
                crate::session_record::record(b"OUT ", written);
            }
            wrote(written);
        })
    }
    fn read(
        &mut self,
        conn: Conn,
        stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        read: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<bool> {
        self.inner.read(conn, stream, pending, &mut |bytes| {
            if conn == Conn::World {
                crate::session_record::record(b"IN  ", bytes);
            }
            read(bytes);
        })
    }
    fn cursor(&mut self, current: i32) -> i32 {
        crate::session_record::cursor(current);
        self.inner.cursor(current)
    }
    fn window_event(&mut self, event: WindowRecord) {
        match &event {
            WindowRecord::Resized([w, h]) => {
                crate::session_record::record(
                    b"RSIZ",
                    &[w.to_le_bytes(), h.to_le_bytes()].concat(),
                );
            }
            WindowRecord::Focused(focused) => {
                crate::session_record::record(b"FOCS", &[u8::from(*focused)]);
            }
            WindowRecord::Input(kind) => crate::session_record::record(b"INPT", &[*kind]),
            WindowRecord::AcceptedInput(input) => match input.encode() {
                Ok(bytes) => crate::session_record::record(RECORD_TAG, &bytes),
                Err(error) => log::warn!("[client910] input record refused: {error:#}"),
            },
        }
        self.inner.window_event(event);
    }
    fn drops_os_input(&self) -> bool {
        crate::session_record::active() && !self.interactive
    }
}

/// A recorded (or scripted) session fed back to the core with no socket
/// and no window: each cycle's recorded clock (`NOWM`, installed as this
/// thread's logic clock from the start of the frame, as the shell's
/// timer poll installs a fixed clock), world bytes (`IN  `) and cursor
/// (`CURS`); the bytes the client writes are captured
/// ([`ReplayIo::take_written`]); the window events (`FOCS`, `RSIZ`) are
/// handed back after their cycle ([`ReplayIo::window_events`]). The
/// transport is `session::{flush_nonblocking, read_nonblocking}` over
/// in-memory streams, so the core sees the same chunking as a loopback
/// socket that has every byte buffered.
///
/// A scripted cycle ([`ReplayIo::set_scripted`], the scenario tests past
/// the recording) or a cycle the recording lacks advances the clock one
/// 20 ms logic interval; a scripted cycle reads only the bytes the test
/// pushed ([`ReplayIo::push_inbound`]) and keeps the last cursor.
#[cfg(any(test, feature = "test-hooks"))]
pub struct ReplayIo {
    records: Vec<crate::session_record::Record>,
    /// Cycles follow the test, not the recording.
    scripted: bool,
    /// The cycle whose clock is installed.
    clock_cycle: Option<i32>,
    /// World bytes the client has not read yet.
    inbound: std::collections::VecDeque<u8>,
    /// World bytes the client wrote since the last [`ReplayIo::take_written`].
    written: Vec<u8>,
    /// Lobby bytes the client wrote (a replay has no lobby server).
    lobby_written: Vec<u8>,
    cursor: i32,
}

#[cfg(any(test, feature = "test-hooks"))]
impl ReplayIo {
    /// A replay of `records` (a decoded RTR1 trace; none for a scripted
    /// session).
    pub fn new(records: Vec<crate::session_record::Record>) -> Self {
        Self {
            records,
            scripted: false,
            clock_cycle: None,
            inbound: Default::default(),
            written: Vec::new(),
            lobby_written: Vec::new(),
            // ViewerApp's cursor state before its first change.
            cursor: i32::MIN,
        }
    }

    /// The recorded `NOWM` of `cycle`.
    pub fn now(&self, cycle: i32) -> Option<i64> {
        self.records
            .iter()
            .find(|r| &r.tag == b"NOWM" && r.cycle == cycle)
            .map(|r| i64::from_le_bytes(r.bytes[..8].try_into().unwrap()))
    }

    /// The recorded payloads of `tag` at `cycle`, concatenated.
    pub fn bytes(&self, tag: &[u8; 4], cycle: i32) -> Vec<u8> {
        self.records
            .iter()
            .filter(|r| &r.tag == tag && r.cycle == cycle)
            .flat_map(|r| r.bytes.iter().copied())
            .collect()
    }

    /// Exactly one phase payload; duplicates are not concatenated into a tape.
    pub fn one_bytes(&self, tag: &[u8; 4], cycle: i32) -> anyhow::Result<&[u8]> {
        let mut records = self
            .records
            .iter()
            .filter(|r| &r.tag == tag && r.cycle == cycle);
        let record = records.next().ok_or_else(|| {
            anyhow::anyhow!("missing {} phase at {cycle}", String::from_utf8_lossy(tag))
        })?;
        anyhow::ensure!(
            records.next().is_none(),
            "duplicate {} phase at {cycle}",
            String::from_utf8_lossy(tag)
        );
        Ok(&record.bytes)
    }

    /// Whether the recording has a `tag` record at `cycle`.
    pub fn has(&self, tag: &[u8; 4], cycle: i32) -> bool {
        self.records
            .iter()
            .any(|r| &r.tag == tag && r.cycle == cycle)
    }

    /// Whether the next cycles are scripted (see the type docs).
    pub fn set_scripted(&mut self, scripted: bool) {
        self.scripted = scripted;
    }

    /// Server bytes for the client's next read (scripted cycles).
    pub fn push_inbound(&mut self, bytes: &[u8]) {
        self.inbound.extend(bytes);
    }

    /// The world bytes the client wrote since the last call.
    pub fn take_written(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.written)
    }

    /// World bytes not read yet.
    pub fn unread(&self) -> usize {
        self.inbound.len()
    }

    /// The window events recorded while `cycle` was the last logic cycle,
    /// which the event loop delivered before the next one.
    pub fn window_events(&self, cycle: i32) -> anyhow::Result<Vec<WindowRecord>> {
        let mut events = Vec::new();
        for record in self.records.iter().filter(|record| record.cycle == cycle) {
            let event = match &record.tag {
                b"FOCS" => {
                    anyhow::ensure!(record.bytes.len() == 1, "malformed focus record");
                    Some(WindowRecord::Focused(record.bytes[0] != 0))
                }
                b"RSIZ" => {
                    anyhow::ensure!(record.bytes.len() == 8, "malformed resize record");
                    Some(WindowRecord::Resized([
                        u32::from_le_bytes(record.bytes[0..4].try_into()?),
                        u32::from_le_bytes(record.bytes[4..8].try_into()?),
                    ]))
                }
                tag if tag == RECORD_TAG => Some(WindowRecord::AcceptedInput(InputEvent::decode(
                    &record.bytes,
                )?)),
                // INPT is historical scripted recording's discarded native input.
                _ => None,
            };
            if let Some(event) = event {
                events.push(event);
            }
        }
        Ok(events)
    }

    /// Install `cycle`'s clock once: its recorded `NOWM`, else one logic
    /// interval after the current time.
    fn install_clock(&mut self, cycle: i32) {
        if self.clock_cycle == Some(cycle) {
            return;
        }
        self.clock_cycle = Some(cycle);
        let recorded = if self.scripted { None } else { self.now(cycle) };
        let now = recorded.unwrap_or_else(|| {
            crate::logic_clock::monotonic_millis() + crate::logic_clock::INTERVAL_NS / 1_000_000
        });
        crate::logic_clock::set_test_now(Some(now));
    }
}

/// An in-memory socket end: reads drain `inbound`, then report
/// `WouldBlock` (the peer is still open); writes append to `out`.
#[cfg(any(test, feature = "test-hooks"))]
struct Buffered<'a> {
    inbound: &'a mut std::collections::VecDeque<u8>,
    out: &'a mut Vec<u8>,
}

#[cfg(any(test, feature = "test-hooks"))]
impl std::io::Read for Buffered<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.inbound.is_empty() {
            return Err(std::io::ErrorKind::WouldBlock.into());
        }
        let n = buf.len().min(self.inbound.len());
        for (slot, byte) in buf.iter_mut().zip(self.inbound.drain(..n)) {
            *slot = byte;
        }
        Ok(n)
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl std::io::Write for Buffered<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(any(test, feature = "test-hooks"))]
impl Io for ReplayIo {
    fn begin_frame(&mut self, next_cycle: i32) {
        self.install_clock(next_cycle);
    }
    fn begin_cycle(&mut self, cycle: i32) {
        self.install_clock(cycle);
        if crate::logic_clock::replaying_monotonic_samples() {
            assert_eq!(
                Some(crate::logic_clock::monotonic_millis()),
                self.now(cycle),
                "recorded P3 sample must equal NOWM"
            );
        }
        if self.scripted {
            return;
        }
        let inbound = self.bytes(b"IN  ", cycle);
        self.inbound.extend(inbound);
        if let Some(record) = self
            .records
            .iter()
            .rfind(|r| &r.tag == b"CURS" && r.cycle == cycle)
        {
            self.cursor = i32::from_le_bytes(record.bytes[..4].try_into().unwrap());
        }
    }
    fn flush(
        &mut self,
        conn: Conn,
        _stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        wrote: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<()> {
        let mut none = std::collections::VecDeque::new();
        let out = match conn {
            Conn::World => &mut self.written,
            Conn::Lobby => &mut self.lobby_written,
        };
        let mut stream = Buffered {
            inbound: &mut none,
            out,
        };
        crate::session::flush_nonblocking(&mut stream, pending, wrote)
    }
    fn read(
        &mut self,
        conn: Conn,
        _stream: &mut WireStream<TcpStream>,
        pending: &mut Vec<u8>,
        read: &mut dyn FnMut(&[u8]),
    ) -> std::io::Result<bool> {
        let mut none = std::collections::VecDeque::new();
        let mut sink = Vec::new();
        let mut stream = match conn {
            Conn::World => Buffered {
                inbound: &mut self.inbound,
                out: &mut sink,
            },
            Conn::Lobby => Buffered {
                inbound: &mut none,
                out: &mut sink,
            },
        };
        crate::session::read_nonblocking(&mut stream, pending, read)
    }
    fn cursor(&mut self, _current: i32) -> i32 {
        self.cursor
    }
    fn window_event(&mut self, _event: WindowRecord) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(cycle: i32, tag: &[u8; 4], bytes: &[u8]) -> crate::session_record::Record {
        crate::session_record::Record {
            cycle,
            tag: *tag,
            bytes: bytes.to_vec(),
        }
    }

    fn loopback() -> (WireStream<TcpStream>, TcpStream) {
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (WireStream::plain(client), server)
    }

    /// The replay transport hands the core each cycle's recorded bytes in
    /// the live transport's chunks, captures what it writes, and installs
    /// the recorded clock and cursor.
    #[test]
    fn replay_io_feeds_recorded_bytes_clock_and_cursor() {
        let big: Vec<u8> = (0..5000u32).map(|i| i as u8).collect();
        let mut io = ReplayIo::new(vec![
            record(1, b"NOWM", &1234i64.to_le_bytes()),
            record(1, b"IN  ", &big),
            record(1, b"CURS", &7i32.to_le_bytes()),
            record(1, b"FOCS", &[0]),
            record(2, b"NOWM", &1254i64.to_le_bytes()),
        ]);
        let (mut stream, _server) = loopback();
        io.begin_cycle(1);
        assert_eq!(crate::logic_clock::monotonic_millis(), 1234);
        let mut chunks = Vec::new();
        let mut pending = Vec::new();
        let closed = io
            .read(Conn::World, &mut stream, &mut pending, &mut |c| {
                chunks.push(c.len())
            })
            .unwrap();
        assert!(!closed);
        assert_eq!(pending, big);
        assert_eq!(chunks, [4096, 904], "read_nonblocking's 4 KB chunks");
        let mut out = vec![1, 2, 3];
        let mut wrote = 0;
        io.flush(Conn::World, &mut stream, &mut out, &mut |c| {
            wrote += c.len()
        })
        .unwrap();
        assert!(out.is_empty());
        assert_eq!((wrote, io.take_written()), (3, vec![1, 2, 3]));
        assert_eq!(io.cursor(-1), 7);
        assert_eq!(io.window_events(1).unwrap(), [WindowRecord::Focused(false)]);
        io.begin_cycle(2);
        assert_eq!(crate::logic_clock::monotonic_millis(), 1254);
        let mut pending = Vec::new();
        assert!(!io
            .read(Conn::World, &mut stream, &mut pending, &mut |_| {})
            .unwrap());
        assert!(pending.is_empty());
        crate::logic_clock::set_test_now(None);
    }

    /// The live transport is the socket's; the recorder adds nothing to it
    /// (and writes nothing while no recording was started).
    #[test]
    fn live_and_recording_io_use_the_socket() {
        use std::io::{Read, Write};
        for recording in [false, true] {
            let mut io: Box<dyn Io> = if recording {
                Box::new(RecordingIo::new(LiveIo))
            } else {
                Box::new(LiveIo)
            };
            let (mut stream, mut server) = loopback();
            stream.set_nonblocking(true).unwrap();
            server.write_all(&[9, 8, 7]).unwrap();
            let mut pending = Vec::new();
            let started = std::time::Instant::now();
            while pending.len() < 3 && started.elapsed() < std::time::Duration::from_secs(5) {
                io.read(Conn::World, &mut stream, &mut pending, &mut |_| {})
                    .unwrap();
            }
            assert_eq!(pending, [9, 8, 7]);
            let mut out = vec![4, 5];
            io.flush(Conn::World, &mut stream, &mut out, &mut |_| {})
                .unwrap();
            assert!(out.is_empty());
            let mut echo = [0u8; 2];
            server.read_exact(&mut echo).unwrap();
            assert_eq!(echo, [4, 5]);
            assert_eq!(io.cursor(11), 11);
            assert!(!io.drops_os_input(), "no recording was started");
        }
    }
}
