//! HTTP content requests, worker scheduling and response decoding.

use std::io::{ErrorKind, Read, Write};

use std::net::TcpStream;

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, Ordering};

use std::sync::{Arc, Mutex};

use std::time::Duration;

use rs910_core::logic_clock;

use super::{Request, RequestRef, HTTP_PENDING_LIMIT};

// ---------------------------------------------------------------------------
// HTTP client
// ---------------------------------------------------------------------------

/// One HTTP request: `padding` zero bytes are
/// appended to the body; `done` marks completion.
pub struct HttpRequest {
    pub(super) padding: usize,
    pub(super) incomplete: AtomicBool,
    pub(super) done: AtomicBool,
    pub(super) response: Mutex<Option<Vec<u8>>>,
}

impl HttpRequest {
    /// `getBytes`: the response of a finished future.
    pub(super) fn bytes(&self) -> Option<Vec<u8>> {
        if !self.done.load(Ordering::Acquire) {
            return None;
        }
        self.response.lock().expect("http response").clone()
    }
}

pub(super) struct HttpJob {
    host: String,
    port: u16,
    path: String,
    request: Arc<HttpRequest>,
}

/// The HTTP client over a two-thread pool.
pub struct HttpClient {
    host: String,
    port: u16,
    game: i32,
    pending: Arc<AtomicI32>,
    last_exception: Arc<AtomicI64>,
    enabled: bool,
    jobs: Option<std::sync::mpsc::Sender<HttpJob>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

impl HttpClient {
    /// A client for `host:port` and game id `game` with a two-thread pool.
    #[must_use]
    pub fn new(host: String, port: u16, game: i32) -> Self {
        let pending = Arc::new(AtomicI32::new(0));
        let last_exception = Arc::new(AtomicI64::new(i64::MIN / 2));
        let (tx, rx) = std::sync::mpsc::channel::<HttpJob>();
        let rx = Arc::new(Mutex::new(rx));
        let mut threads = Vec::new();
        for i in 0..2 {
            let rx = rx.clone();
            let pending = pending.clone();
            let last_exception = last_exception.clone();
            if let Ok(handle) = std::thread::Builder::new()
                .name(format!("client910-js5-http-{i}"))
                .spawn(move || loop {
                    let job = match rx.lock().expect("http jobs").recv() {
                        Ok(job) => job,
                        Err(_) => return,
                    };
                    Self::run(job, &pending, &last_exception);
                })
            {
                threads.push(handle);
            }
        }
        Self {
            host,
            port,
            game,
            pending,
            last_exception,
            enabled: false,
            jobs: Some(tx),
            threads,
        }
    }

    /// Run one HTTP job and complete its request.
    pub(super) fn run(job: HttpJob, pending: &AtomicI32, last_exception: &AtomicI64) {
        let finish = |body: Option<Vec<u8>>| {
            *job.request.response.lock().expect("http response") = body;
            job.request.incomplete.store(false, Ordering::Release);
            pending.fetch_sub(1, Ordering::AcqRel);
            job.request.done.store(true, Ordering::Release);
        };
        match http_get(&job.host, job.port, &job.path) {
            Err(HttpFailure::Connect) => {
                last_exception.store(logic_clock::monotonic_millis(), Ordering::Release);
                finish(None);
            }
            // `getInputStream` threw (a non-2xx status or a broken
            // connection): the future fails, the request never completes
            // and `pendingRequests` is never decremented, as in the original.
            Err(HttpFailure::Response) => job.request.done.store(true, Ordering::Release),
            Ok(mut body) => {
                body.extend(std::iter::repeat_n(0, job.request.padding));
                finish(Some(body));
            }
        }
    }

    #[must_use]
    pub fn is_pending_requests_full(&self) -> bool {
        self.pending.load(Ordering::Acquire) >= HTTP_PENDING_LIMIT
    }

    /// Request the master index over HTTP.
    pub fn request_master_index(&mut self) -> Option<RequestRef> {
        self.send_http_request(255, 255, 0, true, 0, 0)
    }

    /// Send one HTTP request for a group.
    pub fn send_http_request(
        &mut self,
        archive: u32,
        group: u32,
        padding: u8,
        urgent: bool,
        crc: i32,
        version: i32,
    ) -> Option<RequestRef> {
        if self.is_pending_requests_full() {
            return None;
        }
        let master = archive == 255 && group == 255;
        if !self.enabled && !master {
            return None;
        }
        let now = logic_clock::monotonic_millis();
        if self.last_exception.load(Ordering::Acquire) + 10000 >= now {
            return None;
        }
        let tail = if master {
            format!("&cb={now}")
        } else {
            format!("&c={crc}&v={version}")
        };
        let path = format!("/ms?m={}&a={archive}&g={group}{tail}", self.game);
        let request = Arc::new(HttpRequest {
            padding: usize::from(padding),
            incomplete: AtomicBool::new(true),
            done: AtomicBool::new(false),
            response: Mutex::new(None),
        });
        self.pending.fetch_add(1, Ordering::AcqRel);
        let job = HttpJob {
            host: self.host.clone(),
            port: self.port,
            path,
            request: request.clone(),
        };
        if self
            .jobs
            .as_ref()
            .is_some_and(|jobs| jobs.send(job).is_ok())
        {
            Some(Request::http(request, urgent))
        } else {
            self.pending.fetch_sub(1, Ordering::AcqRel);
            None
        }
    }

    pub fn set_http_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// `shutdownExecutor`: queued tasks still run.
    pub fn shutdown(&mut self) {
        self.jobs = None;
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

impl Drop for HttpClient {
    fn drop(&mut self) {
        self.jobs = None;
    }
}

pub(super) enum HttpFailure {
    /// `URLConnection.connect` threw.
    Connect,
    /// `getInputStream` threw.
    Response,
}

/// An HTTP GET: connect timeout
/// 10 s, read timeout 60 s. HTTP/1.1 with `Connection: close`;
/// `Content-Length` and chunked bodies are both read.
pub(super) fn http_get(host: &str, port: u16, path: &str) -> Result<Vec<u8>, HttpFailure> {
    use std::net::ToSocketAddrs;
    let addr = (host, port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.next())
        .ok_or(HttpFailure::Connect)?;
    let mut socket = TcpStream::connect_timeout(&addr, Duration::from_secs(10))
        .map_err(|_| HttpFailure::Connect)?;
    let _ = socket.set_read_timeout(Some(Duration::from_secs(60)));
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: client910\r\nAccept: */*\r\nConnection: close\r\n\r\n"
    );
    socket
        .write_all(request.as_bytes())
        .map_err(|_| HttpFailure::Response)?;
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16384];
    let mut read_failed = false;
    loop {
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            // A read error ends the body.
            Err(_) => {
                read_failed = true;
                break;
            }
        }
    }
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(HttpFailure::Response)?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let mut body = raw[split + 4..].to_vec();
    let mut lines = head.lines();
    let status: u16 = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or(HttpFailure::Response)?;
    if !(200..300).contains(&status) {
        return Err(HttpFailure::Response);
    }
    let headers: Vec<(String, String)> = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_owned()))
        .collect();
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.eq_ignore_ascii_case("chunked"));
    if chunked {
        let mut out = Vec::new();
        let mut pos = 0;
        while let Some(end) = body[pos..].windows(2).position(|w| w == b"\r\n") {
            let size_line = String::from_utf8_lossy(&body[pos..pos + end]).to_string();
            let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("").trim(), 16)
                .unwrap_or(0);
            pos += end + 2;
            if size == 0 || pos + size > body.len() {
                break;
            }
            out.extend_from_slice(&body[pos..pos + size]);
            pos += size + 2;
        }
        body = out;
    } else if let Some(len) = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse::<usize>().ok())
    {
        body.truncate(len);
    }
    let _ = read_failed;
    Ok(body)
}
