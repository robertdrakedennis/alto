//! Archive index and group verification, retry and prefetch scheduling.

use std::collections::{HashMap, VecDeque};

use std::rc::Rc;

use std::sync::Arc;

use anyhow::Context;

use rs910_core::{logic_clock, whirlpool};

use crate::cache::{DiskStore, Stored};

use super::{getcrc, DiskCache, HttpClient, Js5Index, RequestRef, TcpClient};

// ---------------------------------------------------------------------------
// Net resource provider
// ---------------------------------------------------------------------------

/// The shared owners a provider reaches: the TCP client, the HTTP client and
/// the disk cache.
pub struct Net<'a> {
    pub tcp: &'a mut TcpClient,
    pub http: &'a mut HttpClient,
    pub disk: &'a DiskCache,
}

/// The provider of one archive's groups over the network and the disk stores.
pub struct NetResourceProvider {
    pub archive: u32,
    datafs: Option<Arc<DiskStore>>,
    masterfs: Option<Arc<DiskStore>>,
    /// Whether the provider fetches over HTTP.
    http: bool,
    current_request: Option<RequestRef>,
    crc: i32,
    whirlpool: [u8; 64],
    index_version: i32,
    pub index: Option<Arc<Js5Index>>,
    group_status: Vec<i8>,
    verified_groups: i32,
    requests: HashMap<i32, RequestRef>,
    prefetch_all: bool,
    prefetch_requested: bool,
    verify_all: bool,
    group: i32,
    group_queue: Option<VecDeque<i32>>,
    prefetch_queue: VecDeque<i32>,
    discard_orphans: bool,
    orphan_check_time: i64,
}

pub(super) enum Check {
    Ok,
    Failed,
    /// `httpClient != null` with no body: unlink and wait.
    Retry,
}

/// What a [`NetResourceProvider`] is built from: the archive, its stores, the
/// fetch mode and the master index entry its archive index is checked against.
pub struct ProviderSpec {
    pub archive: u32,
    pub datafs: Option<Arc<DiskStore>>,
    pub masterfs: Option<Arc<DiskStore>>,
    /// Whether the provider fetches over HTTP.
    pub http: bool,
    pub discard_orphans: bool,
    /// CRC-32 of the archive index the master index expects.
    pub crc: i32,
    /// Whirlpool digest of the archive index the master index expects.
    pub whirlpool: [u8; 64],
    /// Version of the archive index the master index expects.
    pub index_version: i32,
}

impl NetResourceProvider {
    /// The constructor: a disk store turns `verifyAll` on; the
    /// master store's copy of the index is read at once.
    pub fn new(spec: ProviderSpec, disk: &DiskCache) -> Self {
        let ProviderSpec {
            archive,
            datafs,
            masterfs,
            http,
            discard_orphans,
            crc,
            whirlpool,
            index_version,
        } = spec;
        let verify_all = datafs.is_some();
        let current_request = masterfs
            .as_ref()
            .map(|store| disk.read_synchronous(archive, store));
        Self {
            archive,
            group_queue: verify_all.then(VecDeque::new),
            datafs,
            masterfs,
            http,
            current_request,
            crc,
            whirlpool,
            index_version,
            index: None,
            group_status: Vec::new(),
            verified_groups: 0,
            requests: HashMap::new(),
            prefetch_all: false,
            prefetch_requested: false,
            verify_all,
            group: 0,
            prefetch_queue: VecDeque::new(),
            discard_orphans,
            orphan_check_time: 0,
        }
    }

    /// Request the archive index unless the held one is current.
    pub fn request_index(&mut self, net: &mut Net, crc: i32, whirlpool: [u8; 64], version: i32) {
        if self.is_index_up_to_date(crc, &whirlpool, version) {
            return;
        }
        self.crc = crc;
        self.whirlpool = whirlpool;
        self.index_version = version;
        self.index = None;
        self.current_request = None;
        if !net.tcp.is_urgents_full() {
            self.current_request = Some(net.tcp.queue_request(255, self.archive, 0, true));
        }
    }

    #[must_use]
    pub fn is_index_up_to_date(&self, crc: i32, whirlpool: &[u8; 64], version: i32) -> bool {
        self.crc == crc && self.index_version == version && self.whirlpool == *whirlpool
    }

    /// `getPercentageComplete()`: the index request's progress.
    pub fn index_percentage(&mut self, net: &mut Net) -> i32 {
        if self.fetch_index(net).is_none() {
            self.current_request
                .as_ref()
                .map_or(0, |r| r.borrow().percentage())
        } else {
            100
        }
    }

    /// The archive index once it has been fetched and verified.
    pub fn fetch_index(&mut self, net: &mut Net) -> Option<Arc<Js5Index>> {
        if let Some(index) = &self.index {
            return Some(index.clone());
        }
        if self.current_request.is_none() {
            if net.tcp.is_urgents_full() {
                return None;
            }
            self.current_request = Some(net.tcp.queue_request(255, self.archive, 0, true));
        }
        let request = self.current_request.clone()?;
        if request.borrow().incomplete() {
            return None;
        }
        let bytes = match request.borrow().stored() {
            Some(Stored::Bytes(bytes)) => Some(bytes),
            _ => None,
        };
        let from_disk = request.borrow().is_worker();
        let decoded = bytes
            .as_deref()
            .context("no index bytes")
            .and_then(|b| Js5Index::new(b, self.crc, Some(&self.whirlpool)))
            .and_then(|index| {
                if from_disk && index.index.version != self.index_version {
                    anyhow::bail!(
                        "disk index version {} != {}",
                        index.index.version,
                        self.index_version
                    )
                }
                Ok(index)
            });
        let index = match decoded {
            Ok(index) => index,
            Err(_) => {
                if !from_disk {
                    net.tcp.error(255, self.archive);
                }
                self.index = None;
                self.current_request = if net.tcp.is_urgents_full() {
                    None
                } else {
                    Some(net.tcp.queue_request(255, self.archive, 0, true))
                };
                return None;
            }
        };
        if !from_disk {
            if let (Some(masterfs), Some(bytes)) = (&self.masterfs, bytes) {
                net.disk.write(self.archive, bytes, masterfs);
            }
        }
        self.current_request = None;
        if self.datafs.is_some() {
            self.group_status = vec![0; index.index.capacity];
            self.verified_groups = 0;
        }
        let index = Arc::new(index);
        self.index = Some(index.clone());
        Some(index)
    }

    /// `fetch_group`: the group's bytes once available.
    pub fn fetch_group(&mut self, net: &mut Net, group: i32) -> bool {
        match self.fetch_group_inner(net, group, 0) {
            Some(request) => {
                self.unlink(group, &request);
                true
            }
            None => false,
        }
    }

    pub(super) fn unlink(&mut self, group: i32, request: &RequestRef) {
        if self
            .requests
            .get(&group)
            .is_some_and(|r| Rc::ptr_eq(r, request))
        {
            self.requests.remove(&group);
        }
    }

    /// The urgent re-request after a failed check.
    pub(super) fn requeue_urgent(&mut self, net: &mut Net, index: &Js5Index, group: i32) {
        let g = group as usize;
        if !self.http {
            if !net.tcp.is_urgents_full() {
                let request = net.tcp.queue_request(self.archive, group as u32, 2, true);
                self.requests.insert(group, request);
            }
        } else if !net.http.is_pending_requests_full() {
            if let Some(request) = net.http.send_http_request(
                self.archive,
                group as u32,
                2,
                true,
                index.index.group_checksums[g],
                index.index.group_versions[g],
            ) {
                self.requests.insert(group, request);
            }
        }
    }

    /// `fetch_group_inner(group, mode)`: mode 0 urgent (disk, then
    /// network), 1 queued disk verify, 2 network prefetch.
    pub fn fetch_group_inner(
        &mut self,
        net: &mut Net,
        group: i32,
        mode: i32,
    ) -> Option<RequestRef> {
        let index = self.index.clone()?;
        let g = usize::try_from(group)
            .ok()
            .filter(|&g| g < index.index.capacity)?;
        let mut request = self.requests.get(&group).cloned();
        if let Some(r) = request.clone() {
            let stale = {
                let r = r.borrow();
                mode == 0 && !r.urgent && r.incomplete()
            };
            if stale {
                self.unlink(group, &r);
                request = None;
            }
        }
        let request = match request {
            Some(request) => request,
            None => {
                let created = match mode {
                    0 => {
                        if let Some(datafs) =
                            self.datafs.as_ref().filter(|_| self.group_status[g] != -1)
                        {
                            net.disk.read_synchronous(group as u32, datafs)
                        } else if !self.http {
                            if net.tcp.is_urgents_full() {
                                return None;
                            }
                            net.tcp.queue_request(self.archive, group as u32, 2, true)
                        } else {
                            net.http.send_http_request(
                                self.archive,
                                group as u32,
                                2,
                                true,
                                index.index.group_checksums[g],
                                index.index.group_versions[g],
                            )?
                        }
                    }
                    1 => net.disk.read(group as u32, self.datafs.as_ref()?),
                    2 => {
                        self.datafs.as_ref()?;
                        if self.group_status[g] != -1 || self.http || net.tcp.is_prefetches_full() {
                            return None;
                        }
                        net.tcp.queue_request(self.archive, group as u32, 2, false)
                    }
                    _ => return None,
                };
                self.requests.insert(group, created.clone());
                created
            }
        };
        if request.borrow().incomplete() {
            return None;
        }
        let stored = request.borrow().stored();
        let urgent = request.borrow().urgent;
        let version = index.index.group_versions[g];
        if !request.borrow().is_worker() {
            let check = match &stored {
                Some(Stored::Bytes(bytes)) if bytes.len() > 2 => {
                    if self.bytes_match(&index, g, bytes) {
                        Check::Ok
                    } else {
                        Check::Failed
                    }
                }
                _ if self.http => Check::Retry,
                _ => Check::Failed,
            };
            match check {
                Check::Retry => {
                    self.unlink(group, &request);
                    return None;
                }
                Check::Failed => {
                    net.tcp.error(self.archive, group as u32);
                    self.unlink(group, &request);
                    if urgent {
                        self.requeue_urgent(net, &index, group);
                    }
                    return None;
                }
                Check::Ok => {}
            }
            if self.http {
                net.tcp.error_count = 0;
                net.tcp.js5_state = 0;
            }
            let Some(Stored::Bytes(mut bytes)) = stored else {
                return None;
            };
            let len = bytes.len();
            bytes[len - 2] = (version >> 8) as u8;
            bytes[len - 1] = version as u8;
            request.borrow_mut().set_trailer(version);
            if let Some(datafs) = &self.datafs {
                net.disk.write(group as u32, bytes, datafs);
                if self.group_status[g] != 1 {
                    self.verified_groups += 1;
                    self.group_status[g] = 1;
                }
            }
            if !urgent {
                self.unlink(group, &request);
            }
            return Some(request);
        }
        let ok = match &stored {
            Some(Stored::Bytes(bytes)) if bytes.len() > 2 => {
                let trailer =
                    (i32::from(bytes[bytes.len() - 2]) << 8) + i32::from(bytes[bytes.len() - 1]);
                self.bytes_match(&index, g, bytes) && (version & 0xFFFF) == trailer
            }
            Some(Stored::Recorded {
                crc,
                version: stored_version,
                digest,
            }) => {
                index.index.group_checksums[g] == *crc
                    && Self::digest_matches(&index, g, digest.as_ref())
                    && (version & 0xFFFF) == (stored_version & 0xFFFF)
            }
            _ => false,
        };
        if ok {
            if self.group_status[g] != 1 {
                self.verified_groups += 1;
                self.group_status[g] = 1;
            }
            if !urgent {
                self.unlink(group, &request);
            }
            return Some(request);
        }
        self.group_status[g] = -1;
        self.unlink(group, &request);
        if urgent {
            self.requeue_urgent(net, &index, group);
        }
        None
    }

    /// CRC-32 and Whirlpool over the container without its 2-byte trailer.
    pub(super) fn bytes_match(&self, index: &Js5Index, g: usize, bytes: &[u8]) -> bool {
        let body = &bytes[..bytes.len() - 2];
        if index.index.group_checksums[g] != getcrc(body) {
            return false;
        }
        match index
            .index
            .group_digests
            .as_ref()
            .and_then(|d| d[g].as_ref())
        {
            Some(expected) => whirlpool::compute(body) == *expected,
            None => true,
        }
    }

    pub(super) fn digest_matches(index: &Js5Index, g: usize, digest: Option<&[u8; 64]>) -> bool {
        match index
            .index
            .group_digests
            .as_ref()
            .and_then(|d| d[g].as_ref())
        {
            Some(expected) => digest == Some(expected),
            None => true,
        }
    }

    /// Advance the prefetch queue.
    pub fn process_prefetch_queue(&mut self, net: &mut Net) {
        if self.group_queue.is_none() {
            return;
        }
        let Some(index) = self.fetch_index(net) else {
            return;
        };
        let queue = std::mem::take(&mut self.prefetch_queue);
        let mut kept = VecDeque::with_capacity(queue.len());
        for group in queue {
            let valid = usize::try_from(group)
                .ok()
                .filter(|&g| g < index.index.capacity && index.index.group_sizes[g] != 0);
            let Some(g) = valid else {
                continue;
            };
            if self.group_status[g] == 0 {
                self.fetch_group_inner(net, group, 1);
            }
            if self.group_status[g] == -1 {
                self.fetch_group_inner(net, group, 2);
            }
            if self.group_status[g] != 1 {
                kept.push_back(group);
            }
        }
        kept.extend(std::mem::take(&mut self.prefetch_queue));
        self.prefetch_queue = kept;
    }

    /// `update`: the verify pass, then the prefetch-all pass,
    /// then orphaned-request discard.
    pub fn update(&mut self, net: &mut Net) {
        if self.group_queue.is_some() {
            let Some(index) = self.fetch_index(net) else {
                return;
            };
            let sizes = &index.index.group_sizes;
            if self.verify_all {
                let mut done = true;
                let queue = self.group_queue.take().unwrap_or_default();
                let mut kept = VecDeque::with_capacity(queue.len());
                for group in queue {
                    let g = group as usize;
                    if self.group_status[g] == 0 {
                        self.fetch_group_inner(net, group, 1);
                    }
                    if self.group_status[g] == 0 {
                        done = false;
                        kept.push_back(group);
                    }
                }
                while (self.group as usize) < sizes.len() {
                    let g = self.group as usize;
                    if sizes[g] == 0 {
                        self.group += 1;
                        continue;
                    }
                    if net.disk.pending_requests() >= 250 {
                        done = false;
                        break;
                    }
                    if self.group_status[g] == 0 {
                        self.fetch_group_inner(net, self.group, 1);
                    }
                    if self.group_status[g] == 0 {
                        kept.push_back(self.group);
                        done = false;
                    }
                    self.group += 1;
                }
                self.group_queue = Some(kept);
                if done {
                    self.verify_all = false;
                    self.group = 0;
                }
            } else if self.prefetch_all {
                let mut done = true;
                let queue = self.group_queue.take().unwrap_or_default();
                let mut kept = VecDeque::with_capacity(queue.len());
                for group in queue {
                    let g = group as usize;
                    if self.group_status[g] != 1 {
                        self.fetch_group_inner(net, group, 2);
                    }
                    if self.group_status[g] != 1 {
                        kept.push_back(group);
                        done = false;
                    }
                }
                while (self.group as usize) < sizes.len() {
                    let g = self.group as usize;
                    if sizes[g] == 0 {
                        self.group += 1;
                        continue;
                    }
                    if net.tcp.is_prefetches_full() {
                        done = false;
                        break;
                    }
                    if self.group_status[g] != 1 {
                        self.fetch_group_inner(net, self.group, 2);
                    }
                    if self.group_status[g] != 1 {
                        kept.push_back(self.group);
                        done = false;
                    }
                    self.group += 1;
                }
                self.group_queue = Some(kept);
                if done {
                    self.prefetch_all = false;
                    self.group = 0;
                }
            } else {
                self.group_queue = None;
            }
        }
        let now = logic_clock::monotonic_millis();
        if !self.discard_orphans || now < self.orphan_check_time {
            return;
        }
        let groups: Vec<i32> = self.requests.keys().copied().collect();
        for group in groups {
            let Some(request) = self.requests.get(&group).cloned() else {
                continue;
            };
            let mut r = request.borrow_mut();
            if r.incomplete() {
                continue;
            }
            if r.orphan {
                if !r.urgent {
                    // The original raises here: a finished prefetch that
                    // nobody consumed for a full orphan period.
                    log::info!(
                        "[client910] js5 archive {} group {group}: orphaned prefetch discarded",
                        self.archive
                    );
                }
                drop(r);
                self.unlink(group, &request);
            } else {
                r.orphan = true;
            }
        }
        self.orphan_check_time = now + 1000;
    }

    /// Number of groups in the archive index.
    #[must_use]
    pub fn index_size(&self) -> i32 {
        self.index.as_ref().map_or(0, |i| {
            i32::try_from(i.index.group_count).unwrap_or(i32::MAX)
        })
    }

    /// Number of groups verified so far.
    #[must_use]
    pub fn verified_groups(&self) -> i32 {
        self.verified_groups
    }

    /// Groups loaded so far: the verify cursor (head of the queue)
    /// while verifying, else the index size.
    #[must_use]
    pub fn loaded_groups(&self) -> i32 {
        match &self.index {
            None => 0,
            Some(_) if self.verify_all => self
                .group_queue
                .as_ref()
                .and_then(|q| q.front().copied())
                .unwrap_or(0),
            Some(index) => i32::try_from(index.index.group_count).unwrap_or(i32::MAX),
        }
    }

    /// Request prefetch-all (disk store, no HTTP).
    pub fn request_prefetch_all(&mut self) {
        if self.http || self.datafs.is_none() {
            return;
        }
        self.prefetch_all = true;
        self.prefetch_requested = true;
        if self.group_queue.is_none() {
            self.group_queue = Some(VecDeque::new());
        }
    }

    /// Queue a group for prefetching.
    #[allow(dead_code)]
    pub fn prefetch_group(&mut self, group: i32) {
        if self.datafs.is_none() || self.prefetch_queue.contains(&group) {
            return;
        }
        self.prefetch_queue.push_back(group);
    }

    /// Download progress of one group, in percent.
    #[must_use]
    pub fn group_percentage(&self, group: i32) -> i32 {
        self.requests
            .get(&group)
            .map_or(0, |r| r.borrow().percentage())
    }

    /// Whether prefetch-all was requested.
    #[must_use]
    pub fn prefetch_requested(&self) -> bool {
        self.prefetch_requested
    }

    #[must_use]
    pub fn has_http_client(&self) -> bool {
        self.http
    }

    /// `verifyAll` still running.
    #[must_use]
    #[allow(dead_code)]
    pub fn verifying(&self) -> bool {
        self.verify_all
    }
}
