//! Queued disk-cache reads and writes and the worker shutdown lifecycle.

use std::collections::VecDeque;

use std::sync::{Arc, Condvar, Mutex};

use crate::cache::{DiskStore, Stored};

use super::{Request, RequestRef};

/// A worker request: `type` 1 synchronous
/// read, 2 write, 3 queued read.
pub struct WorkerRequest {
    pub(super) kind: u8,
    pub(super) key: u32,
    pub(super) store: Arc<DiskStore>,
    pub(super) write: Option<Arc<Vec<u8>>>,
    pub(super) state: Mutex<WorkerState>,
}

pub(super) struct WorkerState {
    pub(super) incomplete: bool,
    pub(super) data: Option<Stored>,
}

// ---------------------------------------------------------------------------
// Disk cache
// ---------------------------------------------------------------------------

/// The disk cache: one low-priority thread
/// serving queued reads/writes of the disk stores.
pub struct DiskCache {
    shared: Arc<DiskShared>,
    thread: Option<std::thread::JoinHandle<()>>,
}

pub(super) struct DiskShared {
    queue: Mutex<DiskQueue>,
    wake: Condvar,
}

pub(super) struct DiskQueue {
    queue: VecDeque<Arc<WorkerRequest>>,
    pending: i32,
    stop: bool,
}

impl DiskCache {
    /// Start the daemon disk thread.
    #[must_use]
    pub fn new() -> Self {
        let shared = Arc::new(DiskShared {
            queue: Mutex::new(DiskQueue {
                queue: VecDeque::new(),
                pending: 0,
                stop: false,
            }),
            wake: Condvar::new(),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("client910-js5-disk".into())
            .spawn(move || Self::run(&worker))
            .ok();
        if thread.is_none() {
            log::warn!("[client910] js5 disk thread failed to start; disk reads stay queued");
        }
        Self { shared, thread }
    }

    /// The disk thread body: serve queued requests until told to stop.
    pub(super) fn run(shared: &DiskShared) {
        loop {
            let request = {
                let mut queue = shared.queue.lock().expect("js5 disk queue");
                if queue.stop {
                    return;
                }
                match queue.queue.pop_front() {
                    Some(request) => {
                        queue.pending -= 1;
                        request
                    }
                    None => {
                        let _unused = shared.wake.wait(queue).expect("js5 disk queue");
                        continue;
                    }
                }
            };
            let mut data = None;
            if request.kind == 2 {
                if let Some(bytes) = &request.write {
                    // `diskStore.write` (decompiled as `read(int, byte[], int)`).
                    if let Err(error) = request.store.write(request.key, bytes) {
                        // Reported to the log.
                        log::warn!(
                            "[client910] js5 disk write {}/{} failed: {error}",
                            request.store.archive,
                            request.key
                        );
                    }
                    request.store.overlay().clear_pending(
                        request.store.archive,
                        request.key,
                        bytes,
                    );
                }
            } else if request.kind == 3 {
                data = request.store.read_recorded(request.key);
            }
            let mut state = request.state.lock().expect("worker request");
            if request.kind == 3 {
                state.data = data;
            }
            state.incomplete = false;
        }
    }

    /// `readSynchronous`: a queued write of the key wins, else the
    /// store is read on the calling thread.
    #[must_use]
    pub fn read_synchronous(&self, key: u32, store: &Arc<DiskStore>) -> RequestRef {
        {
            let queue = self.shared.queue.lock().expect("js5 disk queue");
            for queued in &queue.queue {
                if queued.key == key && queued.store.archive == store.archive && queued.kind == 2 {
                    let request = Arc::new(WorkerRequest {
                        kind: 1,
                        key,
                        store: store.clone(),
                        write: None,
                        state: Mutex::new(WorkerState {
                            incomplete: false,
                            data: queued
                                .write
                                .as_ref()
                                .map(|b| Stored::Bytes(b.as_ref().clone())),
                        }),
                    });
                    return Request::worker(request, false);
                }
            }
        }
        let data = store.read(key).map(Stored::Bytes);
        let request = Arc::new(WorkerRequest {
            kind: 1,
            key,
            store: store.clone(),
            write: None,
            state: Mutex::new(WorkerState {
                incomplete: false,
                data,
            }),
        });
        Request::worker(request, true)
    }

    /// Queue a write of `bytes` under `key`.
    pub fn write(&self, key: u32, bytes: Vec<u8>, store: &Arc<DiskStore>) {
        let bytes = Arc::new(bytes);
        store
            .overlay()
            .mark_pending(store.archive, key, bytes.clone());
        self.queue_request(Arc::new(WorkerRequest {
            kind: 2,
            key,
            store: store.clone(),
            write: Some(bytes),
            state: Mutex::new(WorkerState {
                incomplete: true,
                data: None,
            }),
        }));
    }

    /// Queue a read of `key`.
    #[must_use]
    pub fn read(&self, key: u32, store: &Arc<DiskStore>) -> RequestRef {
        let request = Arc::new(WorkerRequest {
            kind: 3,
            key,
            store: store.clone(),
            write: None,
            state: Mutex::new(WorkerState {
                incomplete: true,
                data: None,
            }),
        });
        self.queue_request(request.clone());
        Request::worker(request, false)
    }

    /// Append a request to the disk queue and wake the thread.
    pub(super) fn queue_request(&self, request: Arc<WorkerRequest>) {
        let mut queue = self.shared.queue.lock().expect("js5 disk queue");
        queue.queue.push_back(request);
        queue.pending += 1;
        self.shared.wake.notify_all();
    }

    #[must_use]
    pub fn pending_requests(&self) -> i32 {
        self.shared.queue.lock().expect("js5 disk queue").pending
    }

    /// Stop the disk thread.
    pub fn quit(&mut self) {
        self.shared.queue.lock().expect("js5 disk queue").stop = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Default for DiskCache {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for DiskCache {
    fn drop(&mut self) {
        self.quit();
    }
}
