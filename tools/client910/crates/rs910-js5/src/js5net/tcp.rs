//! Buffered JS5 transport, request queues and chunked response assembly.

use std::collections::VecDeque;

use std::io::{ErrorKind, Read, Write};

use std::net::TcpStream;

use std::rc::Rc;

use std::time::Duration;

use rs910_core::logic_clock;

use super::{
    Request, RequestRef, ENABLE_JS5_XOR, JS5_CHUNK_TOTAL, MAX_CONTAINER_BYTES, OPCODE_JS5_PREFETCH,
    OPCODE_JS5_URGENT, QUEUE_LIMIT,
};

// ---------------------------------------------------------------------------
// Stream (socket stream, reader and writer)
// ---------------------------------------------------------------------------

/// A JS5 socket stream: the reference client's reader/writer threads
/// buffer the socket; here a non-blocking socket is drained into the same
/// bounded buffer whenever the owner asks what is available.
pub struct Js5Stream {
    socket: TcpStream,
    inbound: VecDeque<u8>,
    outbound: Vec<u8>,
    /// The reader's I/O error (EOF or a socket error).
    ioerror: Option<ErrorKind>,
    capacity: usize,
}

impl Js5Stream {
    /// Wrap a non-blocking, no-delay socket with buffers of `capacity` bytes.
    pub fn new(socket: TcpStream, capacity: usize) -> std::io::Result<Self> {
        socket.set_nodelay(true)?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            inbound: VecDeque::new(),
            outbound: Vec::new(),
            ioerror: None,
            capacity,
        })
    }

    pub(super) fn fill(&mut self) {
        let _ = self.flush();
        let mut chunk = [0_u8; 16384];
        while self.ioerror.is_none() && self.inbound.len() < self.capacity {
            let room = (self.capacity - self.inbound.len()).min(chunk.len());
            match self.socket.read(&mut chunk[..room]) {
                Ok(0) => self.ioerror = Some(ErrorKind::UnexpectedEof),
                Ok(n) => self.inbound.extend(&chunk[..n]),
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => self.ioerror = Some(error.kind()),
            }
        }
    }

    /// Bytes buffered for reading; an I/O error surfaces once the buffer is empty.
    pub fn available(&mut self) -> std::io::Result<usize> {
        self.fill();
        if self.inbound.is_empty() {
            if let Some(kind) = self.ioerror {
                return Err(kind.into());
            }
        }
        Ok(self.inbound.len())
    }

    /// Whether at least `n` bytes are buffered.
    pub fn has_available(&mut self, n: usize) -> std::io::Result<bool> {
        self.fill();
        if self.inbound.len() >= n {
            return Ok(true);
        }
        match self.ioerror {
            Some(kind) => Err(kind.into()),
            None => Ok(false),
        }
    }

    /// `read(buf, off, len)` of bytes already available.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.inbound.len());
        for (slot, byte) in out.iter_mut().zip(self.inbound.drain(..n)) {
            *slot = byte;
        }
        n
    }

    /// Queue, then push what the socket takes.
    pub fn write(&mut self, data: &[u8]) -> std::io::Result<()> {
        if let Some(kind) = self.ioerror.filter(|k| *k != ErrorKind::UnexpectedEof) {
            return Err(kind.into());
        }
        self.outbound.extend_from_slice(data);
        self.flush()
    }

    pub(super) fn flush(&mut self) -> std::io::Result<()> {
        while !self.outbound.is_empty() {
            match self.socket.write(&self.outbound) {
                Ok(0) => return Err(ErrorKind::WriteZero.into()),
                Ok(n) => {
                    self.outbound.drain(..n);
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        Ok(())
    }

    /// `closeGracefully`: the writer drains first.
    pub fn close_gracefully(&mut self) {
        let _ = self.socket.set_nonblocking(false);
        let _ = self
            .socket
            .set_write_timeout(Some(Duration::from_millis(500)));
        let pending = std::mem::take(&mut self.outbound);
        let _ = self.socket.write_all(&pending);
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }

    /// `closeForcefully`: drop unsent bytes.
    #[allow(dead_code)]
    pub fn close_forcefully(&mut self) {
        self.outbound.clear();
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

// ---------------------------------------------------------------------------
// TCP client
// ---------------------------------------------------------------------------

/// The JS5 TCP client: request queues, framing and reply parsing.
pub struct TcpClient {
    urgent: VecDeque<RequestRef>,
    urgent_requested: VecDeque<RequestRef>,
    prefetch: VecDeque<RequestRef>,
    prefetch_requested: VecDeque<RequestRef>,
    delay: i32,
    last_timestamp: i64,
    xorcode: u8,
    pub error_count: i32,
    pub js5_state: i32,
    pub archive: i32,
    pub group: i32,
    client: [u8; 5],
    client_pos: usize,
    server: [u8; 5],
    server_pos: usize,
    out_pos: usize,
    current: Option<RequestRef>,
    stream: Option<Js5Stream>,
    /// Bytes read from the content server (diagnostics only).
    pub bytes_in: u64,
}

impl Default for TcpClient {
    fn default() -> Self {
        Self {
            urgent: VecDeque::new(),
            urgent_requested: VecDeque::new(),
            prefetch: VecDeque::new(),
            prefetch_requested: VecDeque::new(),
            delay: 0,
            last_timestamp: 0,
            xorcode: 0,
            error_count: 0,
            js5_state: 0,
            archive: -1,
            group: -1,
            client: [0; 5],
            client_pos: 0,
            server: [0; 5],
            server_pos: 0,
            out_pos: 0,
            current: None,
            stream: None,
            bytes_in: 0,
        }
    }
}

pub(super) fn remove_request(list: &mut VecDeque<RequestRef>, request: &RequestRef) -> bool {
    if let Some(i) = list.iter().position(|r| Rc::ptr_eq(r, request)) {
        list.remove(i);
        true
    } else {
        false
    }
}

impl TcpClient {
    /// Queue a request for one group. The caller checks the matching
    /// `is_*_full` first; queueing into a full queue is a programming error.
    pub fn queue_request(
        &mut self,
        archive: u32,
        group: u32,
        offset: u8,
        urgent: bool,
    ) -> RequestRef {
        let key = (u64::from(archive) << 32) + u64::from(group);
        let request = Request::net(key, usize::from(offset), urgent);
        let list = if urgent {
            assert!(
                self.total_urgents() < QUEUE_LIMIT,
                "JS5 TCP client urgent queue full"
            );
            &mut self.urgent
        } else {
            assert!(
                self.total_prefetches() < QUEUE_LIMIT,
                "JS5 TCP client prefetch queue full"
            );
            &mut self.prefetch
        };
        list.push_back(request.clone());
        request
    }

    #[must_use]
    pub fn is_prefetches_full(&self) -> bool {
        self.total_prefetches() >= QUEUE_LIMIT
    }

    #[must_use]
    pub fn is_urgents_full(&self) -> bool {
        self.total_urgents() >= QUEUE_LIMIT
    }

    #[must_use]
    pub fn total_prefetches(&self) -> usize {
        self.prefetch.len() + self.prefetch_requested.len()
    }

    #[must_use]
    pub fn total_urgents(&self) -> usize {
        self.urgent.len() + self.urgent_requested.len()
    }

    /// True while a content stream is open.
    #[must_use]
    #[allow(dead_code)]
    pub fn connected(&self) -> bool {
        self.stream.is_some()
    }

    pub(super) fn drop_stream(&mut self) {
        if let Some(mut stream) = self.stream.take() {
            stream.close_gracefully();
        }
    }

    pub(super) fn io_failed(&mut self) {
        self.drop_stream();
        self.error_count += 1;
        self.js5_state = -2;
    }

    /// `error(archive, group)`: a group
    /// failed its checks; drop the stream so the next connect re-requests.
    pub fn error(&mut self, archive: u32, group: u32) {
        self.drop_stream();
        self.error_count += 1;
        self.js5_state = -1;
        let nanos = logic_clock::system_time()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        self.xorcode = (nanos % 255 + 1) as u8;
        self.archive = archive as i32;
        self.group = group as i32;
    }

    /// `process()`: false when the stream
    /// failed or is gone while requests wait.
    pub fn process(&mut self) -> bool {
        if self.stream.is_some() {
            let now = logic_clock::monotonic_millis();
            let elapsed = (now - self.last_timestamp).min(200) as i32;
            self.last_timestamp = now;
            self.delay += elapsed;
            if self.delay > 30000 {
                self.drop_stream();
            }
        }
        if self.stream.is_none() {
            return self.total_urgents() == 0 && self.total_prefetches() == 0;
        }
        match self.process_stream() {
            Ok(()) => true,
            Err(_) => {
                self.io_failed();
                self.total_urgents() == 0 && self.total_prefetches() == 0
            }
        }
    }

    pub(super) fn process_stream(&mut self) -> std::io::Result<()> {
        let stream = self.stream.as_mut().expect("js5 stream");
        while let Some(request) = self.urgent.pop_front() {
            let key = request.borrow().net_key().unwrap_or(0);
            let mut out = [0_u8; 6];
            out[0] = OPCODE_JS5_URGENT;
            out[1..].copy_from_slice(&key.to_be_bytes()[3..]);
            stream.write(&out)?;
            self.urgent_requested.push_back(request);
        }
        while let Some(request) = self.prefetch.pop_front() {
            let key = request.borrow().net_key().unwrap_or(0);
            let mut out = [0_u8; 6];
            out[0] = OPCODE_JS5_PREFETCH;
            out[1..].copy_from_slice(&key.to_be_bytes()[3..]);
            stream.write(&out)?;
            self.prefetch_requested.push_back(request);
        }
        for _ in 0..100 {
            let stream = self.stream.as_mut().expect("js5 stream");
            let available = stream.available()?;
            if available == 0 {
                break;
            }
            self.delay = 0;
            match self.current.clone() {
                None => {
                    let n = (5 - self.client_pos).min(available);
                    let read = stream.read(&mut self.client[self.client_pos..self.client_pos + n]);
                    self.bytes_in += read as u64;
                    if self.xorcode != 0 && ENABLE_JS5_XOR {
                        for b in &mut self.client[self.client_pos..self.client_pos + n] {
                            *b ^= self.xorcode;
                        }
                    }
                    self.client_pos += n;
                    if self.client_pos >= 5 {
                        self.client_pos = 0;
                        let archive = u64::from(self.client[0]);
                        let value = i32::from_be_bytes([
                            self.client[1],
                            self.client[2],
                            self.client[3],
                            self.client[4],
                        ]);
                        let prefetch = value & i32::MIN != 0;
                        let group = u64::from((value & i32::MAX) as u32);
                        let key = (archive << 32) + group;
                        let list = if prefetch {
                            &self.prefetch_requested
                        } else {
                            &self.urgent_requested
                        };
                        self.current = list
                            .iter()
                            .find(|r| r.borrow().net_key() == Some(key))
                            .cloned();
                        if self.current.is_none() {
                            return Err(std::io::Error::new(
                                ErrorKind::InvalidData,
                                format!("js5 reply for unrequested {archive}/{group}"),
                            ));
                        }
                        self.out_pos = 5;
                        self.client_pos = 0;
                        self.server_pos = 0;
                    }
                }
                Some(current) => {
                    let mut request = current.borrow_mut();
                    let net = request.net_mut().expect("js5 net request");
                    match net.buf.as_mut() {
                        None => {
                            let n = (5 - self.server_pos).min(available);
                            let read =
                                stream.read(&mut self.server[self.server_pos..self.server_pos + n]);
                            self.bytes_in += read as u64;
                            if self.xorcode != 0 && ENABLE_JS5_XOR {
                                for b in &mut self.server[self.server_pos..self.server_pos + n] {
                                    *b ^= self.xorcode;
                                }
                            }
                            self.server_pos += n;
                            if self.server_pos >= 5 {
                                self.server_pos = 0;
                                let compression = self.server[0];
                                let length = i32::from_be_bytes([
                                    self.server[1],
                                    self.server[2],
                                    self.server[3],
                                    self.server[4],
                                ]);
                                let header = if compression == 0 { 5 } else { 9 };
                                // The original allocates `length + header +
                                // offset` bytes; a corrupt length is its allocation
                                // failure, handled here as a stream error.
                                let total = usize::try_from(length)
                                    .ok()
                                    .filter(|&l| l <= MAX_CONTAINER_BYTES)
                                    .ok_or_else(|| {
                                        std::io::Error::new(
                                            ErrorKind::InvalidData,
                                            format!("js5 container length {length}"),
                                        )
                                    })?
                                    + header
                                    + net.offset;
                                let mut buf = vec![0_u8; total];
                                buf[0] = compression;
                                buf[1..5].copy_from_slice(&self.server[1..5]);
                                net.buf = Some(buf);
                                net.pos = 5;
                                self.out_pos += 5;
                            }
                        }
                        Some(buf) => {
                            let target = buf.len() - net.offset;
                            let n = (JS5_CHUNK_TOTAL - self.out_pos)
                                .min(target - net.pos)
                                .min(available);
                            let read = stream.read(&mut buf[net.pos..net.pos + n]);
                            self.bytes_in += read as u64;
                            if self.xorcode != 0 && ENABLE_JS5_XOR {
                                for b in &mut buf[net.pos..net.pos + n] {
                                    *b ^= self.xorcode;
                                }
                            }
                            net.pos += n;
                            self.out_pos += n;
                            if net.pos == target {
                                net.incomplete = false;
                                drop(request);
                                if !remove_request(&mut self.urgent_requested, &current) {
                                    remove_request(&mut self.prefetch_requested, &current);
                                }
                                self.current = None;
                            } else if self.out_pos == JS5_CHUNK_TOTAL {
                                self.out_pos = 0;
                                self.current = None;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// `createNewJs5Stream(stream, loggedIn)`: replace the stream,
    /// announce it, and re-queue every request already sent.
    pub fn create_new_stream(&mut self, stream: Js5Stream, logged_in: bool) {
        self.drop_stream();
        self.stream = Some(stream);
        self.send_new_stream();
        self.send_login_status(logged_in);
        self.client_pos = 0;
        self.server_pos = 0;
        self.current = None;
        while let Some(request) = self.urgent_requested.pop_front() {
            if let Some(net) = request.borrow_mut().net_mut() {
                net.buf = None;
                net.pos = 0;
            }
            self.urgent.push_back(request);
        }
        while let Some(request) = self.prefetch_requested.pop_front() {
            if let Some(net) = request.borrow_mut().net_mut() {
                net.buf = None;
                net.pos = 0;
            }
            self.prefetch.push_back(request);
        }
        if self.xorcode != 0 && ENABLE_JS5_XOR {
            let mut out = [0_u8; 6];
            out[0] = 4;
            out[1] = self.xorcode;
            self.send(&out);
        }
        self.delay = 0;
        self.last_timestamp = logic_clock::monotonic_millis();
    }

    pub(super) fn send(&mut self, out: &[u8; 6]) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        if stream.write(out).is_err() {
            self.io_failed();
        }
    }

    /// `sendNewStream`: `p1(6) p3(4) p2(0)`.
    pub(super) fn send_new_stream(&mut self) {
        self.send(&[6, 0, 0, 4, 0, 0]);
    }

    /// `sendLoginStatus(loggedIn)`: `p1(loggedIn ? 2 : 3) p5(0)`.
    pub fn send_login_status(&mut self, logged_in: bool) {
        self.send(&[if logged_in { 2 } else { 3 }, 0, 0, 0, 0, 0]);
    }

    /// `sendCloseStream`: `p1(7) p5(0)`.
    pub fn send_close_stream(&mut self) {
        self.send(&[7, 0, 0, 0, 0, 0]);
    }

    /// Close after the writer has drained.
    pub fn close_gracefully(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            stream.close_gracefully();
        }
    }

    /// Close immediately, dropping unsent bytes.
    #[allow(dead_code)]
    pub fn close_forcefully(&mut self) {
        if let Some(stream) = self.stream.as_mut() {
            stream.close_forcefully();
        }
    }
}
