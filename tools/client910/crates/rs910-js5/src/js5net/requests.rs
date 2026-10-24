//! Request handles shared by the network transports and disk worker.

use std::cell::RefCell;

use std::rc::Rc;

use std::sync::atomic::Ordering;

use std::sync::Arc;

use crate::cache::Stored;

use super::{HttpRequest, WorkerRequest};

// ---------------------------------------------------------------------------
// Requests: network, disk-worker and HTTP
// ---------------------------------------------------------------------------

/// A shared request node: linked from a provider's `requests` table
/// and, for network requests, from the TCP client's queues.
pub type RequestRef = Rc<RefCell<Request>>;

/// A JS5 request: a network, disk-worker or HTTP fetch of one group.
pub struct Request {
    pub urgent: bool,
    pub orphan: bool,
    body: Body,
}

pub(super) enum Body {
    Net(NetBody),
    Worker(Arc<WorkerRequest>),
    Http(Arc<HttpRequest>),
}

/// A network request: `secondaryNodeId`, `offset`
/// (the trailer space allocated after the container), `buf`.
pub(super) struct NetBody {
    pub(super) key: u64,
    pub(super) offset: usize,
    pub(super) buf: Option<Vec<u8>>,
    pub(super) pos: usize,
    pub(super) incomplete: bool,
}

impl Request {
    pub(super) fn net(key: u64, offset: usize, urgent: bool) -> RequestRef {
        Rc::new(RefCell::new(Self {
            urgent,
            orphan: false,
            body: Body::Net(NetBody {
                key,
                offset,
                buf: None,
                pos: 0,
                incomplete: true,
            }),
        }))
    }

    pub(super) fn worker(request: Arc<WorkerRequest>, urgent: bool) -> RequestRef {
        Rc::new(RefCell::new(Self {
            urgent,
            orphan: false,
            body: Body::Worker(request),
        }))
    }

    pub(super) fn http(request: Arc<HttpRequest>, urgent: bool) -> RequestRef {
        Rc::new(RefCell::new(Self {
            urgent,
            orphan: false,
            body: Body::Http(request),
        }))
    }

    /// Whether the request has not finished yet.
    #[must_use]
    pub fn incomplete(&self) -> bool {
        match &self.body {
            Body::Net(net) => net.incomplete,
            Body::Worker(worker) => worker.state.lock().expect("worker request").incomplete,
            Body::Http(http) => http.incomplete.load(Ordering::Acquire),
        }
    }

    pub(super) fn is_worker(&self) -> bool {
        matches!(self.body, Body::Worker(_))
    }

    /// The bytes of a complete request: the whole buffer (trailer space
    /// included) for a network request, the read data for a worker request,
    /// the response body for an HTTP request.
    pub(super) fn stored(&self) -> Option<Stored> {
        match &self.body {
            Body::Net(net) => net.buf.clone().map(Stored::Bytes),
            Body::Worker(worker) => worker.state.lock().expect("worker request").data.clone(),
            Body::Http(http) => http.bytes().map(Stored::Bytes),
        }
    }

    /// Progress of the request, in percent.
    #[must_use]
    pub fn percentage(&self) -> i32 {
        match &self.body {
            Body::Net(net) => match &net.buf {
                None => 0,
                Some(buf) => {
                    let denom = buf.len().saturating_sub(net.offset).max(1);
                    (net.pos * 100 / denom) as i32
                }
            },
            Body::Worker(_) => {
                if self.incomplete() {
                    0
                } else {
                    100
                }
            }
            Body::Http(http) => {
                if http.done.load(Ordering::Acquire) {
                    100
                } else {
                    0
                }
            }
        }
    }

    pub(super) fn net_key(&self) -> Option<u64> {
        match &self.body {
            Body::Net(net) => Some(net.key),
            _ => None,
        }
    }

    pub(super) fn net_mut(&mut self) -> Option<&mut NetBody> {
        match &mut self.body {
            Body::Net(net) => Some(net),
            _ => None,
        }
    }

    /// Write the version trailer into the request's own buffer
    /// (the trailer is patched in place).
    pub(super) fn set_trailer(&mut self, version: i32) {
        if let Body::Net(NetBody { buf: Some(buf), .. }) = &mut self.body {
            let len = buf.len();
            if len >= 2 {
                buf[len - 2] = (version >> 8) as u8;
                buf[len - 1] = version as u8;
            }
        }
    }
}
