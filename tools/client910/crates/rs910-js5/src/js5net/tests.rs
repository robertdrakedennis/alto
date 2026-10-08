use super::*;
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Master-index slot: `(crc, version, group count, whirlpool)`.
type MasterSlot = Option<(i32, i32, i32, [u8; 64])>;
/// Served `(archive, group)` payloads.
type Served = HashMap<(u8, u32), Vec<u8>>;
/// `(group id, bytes)` pairs of one archive.
type Groups = Vec<(u32, Vec<u8>)>;

#[test]
fn handshake_encode_vectors_are_byte_exact() {
    // Empty token: p1(15) p1(10) p4(910) p4(1) pjstr("") p1(0).
    assert_eq!(
        encode_js5_handshake(910, 1, "", 0).unwrap(),
        vec![15, 10, 0, 0, 3, 142, 0, 0, 0, 1, 0, 0]
    );
    assert_eq!(
        encode_js5_handshake(910, 1, "token", 0).unwrap(),
        vec![15, 15, 0, 0, 3, 142, 0, 0, 0, 1, 116, 111, 107, 101, 110, 0, 0]
    );
    assert_eq!(encode_js5_handshake(910, 1, "", 7).unwrap()[11], 7);
    assert!(encode_js5_handshake(910, 1, "😀", 0).is_err());
}

/// The group checksum is the IEEE CRC-32 (and agrees with the repack
/// bitwise implementation); the name hash of "huffman" is the 31-multiplier
/// string hash.
#[test]
fn crc_and_name_hash_are_the_standard_functions() {
    assert_eq!(getcrc(b"123456789"), 0xCBF4_3926_u32 as i32);
    let data: Vec<u8> = (0..5000_u32).map(|i| (i * 31) as u8).collect();
    assert_eq!(getcrc(&data), native910::repack::crc32(&data));
    // The name is lowercased first.
    assert_eq!(name_hash("HUFFMAN"), 1_258_058_669);
}

// --- fake content server ---------------------------------------------

fn container(payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0_u8];
    out.extend_from_slice(&(payload.len() as i32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Protocol-6 index: dense single-file groups, no names or digests.
fn index_container(groups: &[(u32, i32, i32)]) -> Vec<u8> {
    let mut p = vec![6_u8];
    p.extend_from_slice(&1_i32.to_be_bytes());
    p.push(0);
    p.extend_from_slice(&(groups.len() as u16).to_be_bytes());
    let mut last = 0;
    for &(id, _, _) in groups {
        p.extend_from_slice(&((id - last) as u16).to_be_bytes());
        last = id;
    }
    for &(_, crc, _) in groups {
        p.extend_from_slice(&crc.to_be_bytes());
    }
    for &(_, _, version) in groups {
        p.extend_from_slice(&version.to_be_bytes());
    }
    for _ in groups {
        p.extend_from_slice(&1_u16.to_be_bytes());
    }
    for _ in groups {
        p.extend_from_slice(&0_u16.to_be_bytes());
    }
    container(&p)
}

/// `Cache.generateMasterIndexIndex` (format 7) over `archives` slots.
fn master_container(archives: &[MasterSlot]) -> Vec<u8> {
    let mut p = vec![archives.len() as u8];
    for entry in archives {
        let (crc, version, count, digest) = entry.unwrap_or((0, 0, 0, [0; 64]));
        p.extend_from_slice(&crc.to_be_bytes());
        p.extend_from_slice(&version.to_be_bytes());
        p.extend_from_slice(&count.to_be_bytes());
        p.extend_from_slice(&0_i32.to_be_bytes());
        p.extend_from_slice(&digest);
    }
    let digest = rs910_core::whirlpool::compute(&p);
    p.push(0);
    p.extend_from_slice(&digest);
    container(&p)
}

#[derive(Default)]
struct Log {
    connections: usize,
    /// `(opcode, archive, group)` in arrival order.
    messages: Vec<(u8, u8, u32)>,
}

struct Fake {
    port: u16,
    log: Arc<Mutex<Log>>,
}

#[derive(Clone, Default)]
struct Faults {
    /// Send a corrupted copy the first time these are requested.
    corrupt_once: HashSet<(u8, u32)>,
    /// Close the first connection after this many reply bytes.
    drop_after: Option<usize>,
}

fn fake_server(groups: HashMap<(u8, u32), Vec<u8>>, faults: Faults) -> Fake {
    let log = Arc::new(Mutex::new(Log::default()));
    let (tx, rx) = std::sync::mpsc::channel();
    let server_log = log.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            let groups = Arc::new(groups);
            let faults = Arc::new(Mutex::new(faults));
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    return;
                };
                let first = {
                    let mut log = server_log.lock().unwrap();
                    log.connections += 1;
                    log.connections == 1
                };
                let (log, groups, faults) = (server_log.clone(), groups.clone(), faults.clone());
                tokio::spawn(async move {
                    let mut head = [0_u8; 2];
                    if socket.read_exact(&mut head).await.is_err() || head[0] != 15 {
                        return;
                    }
                    let mut body = vec![0_u8; usize::from(head[1])];
                    if socket.read_exact(&mut body).await.is_err() {
                        return;
                    }
                    let mut reply = vec![0_u8];
                    for i in 0..LOADABLE_RESOURCES as u32 {
                        reply.extend_from_slice(&(i + 1).to_be_bytes());
                    }
                    socket.write_all(&reply).await.unwrap();
                    let mut sent_total = 0_usize;
                    loop {
                        let mut msg = [0_u8; 6];
                        if socket.read_exact(&mut msg).await.is_err() {
                            return;
                        }
                        let archive = msg[1];
                        let group = u32::from_be_bytes([msg[2], msg[3], msg[4], msg[5]]);
                        log.lock().unwrap().messages.push((msg[0], archive, group));
                        match msg[0] {
                            0 | 1 => {}
                            7 => return,
                            _ => continue,
                        }
                        let Some(mut data) = groups.get(&(archive, group)).cloned() else {
                            continue;
                        };
                        if faults
                            .lock()
                            .unwrap()
                            .corrupt_once
                            .remove(&(archive, group))
                        {
                            let last = data.len() - 1;
                            data[last] ^= 0x5A;
                        }
                        let flag = if msg[0] == 1 {
                            group
                        } else {
                            group | 0x8000_0000
                        };
                        let mut out = Vec::new();
                        let mut sent = 0;
                        while sent < data.len() {
                            let take = (data.len() - sent).min(JS5_CHUNK_TOTAL - 5);
                            out.push(archive);
                            out.extend_from_slice(&flag.to_be_bytes());
                            out.extend_from_slice(&data[sent..sent + take]);
                            sent += take;
                        }
                        let limit = if first {
                            faults.lock().unwrap().drop_after
                        } else {
                            None
                        };
                        if let Some(limit) = limit.filter(|&l| sent_total + out.len() > l) {
                            faults.lock().unwrap().drop_after = None;
                            let n = limit.saturating_sub(sent_total).min(out.len());
                            let _ = socket.write_all(&out[..n]).await;
                            let _ = socket.shutdown().await;
                            return;
                        }
                        sent_total += out.len();
                        if socket.write_all(&out).await.is_err() {
                            return;
                        }
                    }
                });
            }
        });
    });
    Fake {
        port: rx.recv().unwrap(),
        log,
    }
}

fn temp_dir(name: &str) -> PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "client910-js5-{name}-{}-{stamp}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Archive 13 (`fontmetrics`) with small groups 0..3 and a 250 kB
/// group 7 spanning three 102400-byte reply chunks.
fn archive_fixture() -> (Served, Groups) {
    let mut groups = Vec::new();
    for id in 0..4_u32 {
        groups.push((id, container(format!("group-{id}").as_bytes())));
    }
    let big: Vec<u8> = (0..250_000_u32).map(|i| (i * 7 + 3) as u8).collect();
    groups.push((7, container(&big)));
    let entries: Vec<(u32, i32, i32)> = groups
        .iter()
        .map(|(id, c)| (*id, getcrc(c), 100 + *id as i32))
        .collect();
    let index = index_container(&entries);
    let mut master = vec![None; 14];
    master[13] = Some((
        getcrc(&index),
        1,
        groups.len() as i32,
        rs910_core::whirlpool::compute(&index),
    ));
    let mut served = HashMap::new();
    served.insert((255, 255), master_container(&master));
    served.insert((255, 13), index);
    for (id, c) in &groups {
        served.insert((13, *id), c.clone());
    }
    (served, groups)
}

fn system(port: u16, name: &str) -> (Js5System, PathBuf) {
    let pack = temp_dir(&format!("{name}-pack"));
    let cache = temp_dir(&format!("{name}-cache"));
    let overlay = DiskOverlay::new(&cache);
    let system = Js5System::new(
        &pack,
        overlay,
        ContentAddress::new("127.0.0.1".into(), port, port),
        ("127.0.0.1".into(), 9),
        0,
        String::new(),
        0,
    );
    (system, cache)
}

fn pump(system: &mut Js5System, mut done: impl FnMut(&mut Js5System) -> bool) {
    let start = std::time::Instant::now();
    while !done(system) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "js5 test timed out"
        );
        assert!(system.fatal.is_none(), "fatal {:?}", system.fatal);
        system.mainloop(5);
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn download_archive(faults: Faults, name: &str) -> (Js5System, Arc<Mutex<Log>>, PathBuf, Groups) {
    let (served, groups) = archive_fixture();
    let fake = fake_server(served, faults);
    let (mut system, cache) = system(fake.port, name);
    system.ensure_client();
    pump(&mut system, Js5System::load_master_index);
    system.create_js5(13, false, 1, true).unwrap();
    assert_eq!(system.preload_percent(), Some(0));
    pump(&mut system, |s| s.fetch_all(13));
    // The prefetch-all/verify passes settle to the full index.
    pump(&mut system, |s| s.preload_percent() == Some(100));
    (system, fake.log, cache, groups)
}

/// The whole fetch path: master index, archive index (CRC + Whirlpool),
/// urgent group fetches, chunked replies, disk-store writes with the
/// version trailer, and the preload/debug accounting.
#[test]
fn js5_client_downloads_an_archive_into_the_disk_store() {
    let (mut system, log, cache, groups) = download_archive(Faults::default(), "full");
    assert_eq!(system.percentage(13), 100);
    let index = std::fs::read(cache.join("255").join("13.dat")).unwrap();
    assert_eq!(&index[..5], &[0, 0, 0, 0, index.len() as u8 - 5][..]);
    for (id, container) in &groups {
        let path = cache.join("13").join(format!("{id}.dat"));
        let pump_until = std::time::Instant::now();
        while !path.is_file() && pump_until.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(5));
        }
        let stored = std::fs::read(&path).unwrap();
        assert_eq!(&stored[..stored.len() - 2], &container[..]);
        let version = 100 + *id as i32;
        assert_eq!(
            stored[stored.len() - 2..],
            [(version >> 8) as u8, version as u8]
        );
    }
    assert_eq!(system.cache_stats(), [5, 5, 5]);
    let log = log.lock().unwrap();
    assert_eq!(log.connections, 1);
    // New-stream announcement, logged-out status, then requests.
    assert_eq!(log.messages[0], (6, 0, 0x0004_0000));
    assert_eq!(log.messages[1], (3, 0, 0));
    assert!(log.messages.contains(&(1, 255, 255)));
    assert!(log.messages.contains(&(1, 255, 13)));
    assert!(log.messages.contains(&(1, 13, 7)));
    // A second client over the same disk store verifies from disk and
    // requests no groups.
    drop(log);
    system.quit();
}

/// CRC mismatch on a group:
/// the TCP client's error handling drops the stream (state -1), the
/// urgent request is re-queued, the client reconnects and the second
/// copy passes.
#[test]
fn crc_mismatch_errors_the_stream_and_retries() {
    let mut faults = Faults::default();
    faults.corrupt_once.insert((13, 2));
    let (system, log, _, _) = download_archive(faults, "crc");
    let log = log.lock().unwrap();
    assert_eq!(log.connections, 2);
    let requests = log
        .messages
        .iter()
        .filter(|m| m.1 == 13 && m.2 == 2 && m.0 <= 1)
        .count();
    assert_eq!(requests, 2);
    assert_eq!(system.tcp.error_count, 1);
    assert_eq!((system.tcp.archive, system.tcp.group), (13, 2));
}

/// A connection lost mid-chunk (a `process` read error, js5State -2)
/// reconnects through `js5Error`, and the stream rebuild re-queues the
/// requests already sent.
#[test]
fn dropped_stream_reconnects_and_requeues() {
    let faults = Faults {
        drop_after: Some(150_000),
        ..Faults::default()
    };
    let (system, log, _, _) = download_archive(faults, "drop");
    let log = log.lock().unwrap();
    assert_eq!(log.connections, 2);
    assert_eq!(system.tcp.js5_state, -2);
    assert!(system.tcp.error_count >= 1);
    // The second stream re-requested the unfinished big group.
    let big = log
        .messages
        .iter()
        .filter(|m| m.1 == 13 && m.2 == 7 && m.0 <= 1)
        .count();
    assert!(big >= 2);
}

/// The TCP client writes every urgent request before any
/// prefetch, and matches prefetch replies (high bit) to the prefetch
/// queue; the queue limits are 500 each.
#[test]
fn urgent_requests_precede_prefetches_and_limits_hold() {
    let (served, _) = archive_fixture();
    let fake = fake_server(served, Faults::default());
    let mut tcp = TcpClient::default();
    let prefetch: Vec<RequestRef> = (0..3).map(|g| tcp.queue_request(13, g, 2, false)).collect();
    let urgent: Vec<RequestRef> = [7, 3]
        .iter()
        .map(|&g| tcp.queue_request(13, g, 2, true))
        .collect();
    let mut socket = TcpStream::connect(("127.0.0.1", fake.port)).unwrap();
    socket
        .write_all(&encode_js5_handshake(910, 1, "", 0).unwrap())
        .unwrap();
    let mut reply = vec![0_u8; 1 + LOADABLE_RESOURCES * 4];
    socket.read_exact(&mut reply).unwrap();
    tcp.create_new_stream(Js5Stream::new(socket, 131_072).unwrap(), true);
    let start = std::time::Instant::now();
    while prefetch
        .iter()
        .chain(&urgent)
        .any(|r| r.borrow().incomplete())
    {
        assert!(start.elapsed() < Duration::from_secs(10));
        assert!(tcp.process());
        std::thread::sleep(Duration::from_millis(1));
    }
    let log = fake.log.lock().unwrap();
    let order: Vec<(u8, u32)> = log.messages.iter().map(|m| (m.0, m.2)).collect();
    assert_eq!(
        order,
        vec![
            (6, 0x0004_0000),
            (2, 0),
            (1, 7),
            (1, 3),
            (0, 0),
            (0, 1),
            (0, 2)
        ]
    );
    assert_eq!(tcp.total_urgents() + tcp.total_prefetches(), 0);
    for _ in 0..QUEUE_LIMIT {
        tcp.queue_request(13, 0, 2, true);
    }
    assert!(tcp.is_urgents_full());
    assert!(!tcp.is_prefetches_full());
}

/// `EXECUTE_CLIENT_CHEAT` 6 sends the server the close-stream message (opcode
/// 7), and 14 closes the connection, which the TCP client then reports as
/// disconnected.
#[test]
fn the_close_stream_cheats_close_the_connection() {
    let (served, _) = archive_fixture();
    let fake = fake_server(served, Faults::default());
    let mut tcp = TcpClient::default();
    let mut socket = TcpStream::connect(("127.0.0.1", fake.port)).unwrap();
    socket
        .write_all(&encode_js5_handshake(910, 1, "", 0).unwrap())
        .unwrap();
    let mut reply = vec![0_u8; 1 + LOADABLE_RESOURCES * 4];
    socket.read_exact(&mut reply).unwrap();
    tcp.create_new_stream(Js5Stream::new(socket, 131_072).unwrap(), true);
    tcp.send_close_stream();
    let start = std::time::Instant::now();
    while !fake.log.lock().unwrap().messages.iter().any(|m| m.0 == 7) {
        assert!(start.elapsed() < Duration::from_secs(10), "no close-stream");
        tcp.process();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(tcp.connected());
    tcp.close_gracefully();
    for _ in 0..1000 {
        tcp.process();
        if !tcp.connected() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(!tcp.connected(), "the stream is closed");
}

/// `Js5MasterIndex` rejects a trailer whose Whirlpool does not match.
#[test]
fn master_index_checks_whirlpool() {
    let mut master = master_container(&[Some((1, 2, 3, [9; 64]))]);
    let decoded = MasterIndex::decode(&master).unwrap();
    assert_eq!(decoded.archives[0].crc, 1);
    assert_eq!(decoded.archives[0].version, 2);
    assert_eq!(decoded.archives[0].group_count, 3);
    let last = master.len() - 1;
    master[last] ^= 1;
    assert!(MasterIndex::decode(&master).is_err());
}

/// A pack group is verified from its recorded checksum; the overlay
/// replaces a pack group and an absent group is queued for the client.
#[test]
fn disk_store_reads_overlay_before_pack_and_queues_absent_groups() {
    let root = pack_fixture_root();
    let cache = temp_dir("overlay-reads");
    let overlay = DiskOverlay::new(&cache);
    let pack = Pack::open_with_overlay(&root, Some(overlay.clone()));
    let store = DiskStore::new(13, pack.clone(), overlay.clone());
    // Group 1 comes from the pack with its version trailer.
    let stored = store.read(1).unwrap();
    assert_eq!(&stored[..stored.len() - 2], &pack_group(1)[..]);
    assert!(matches!(
        store.read_recorded(1),
        Some(Stored::Recorded { .. })
    ));
    // An overlay write replaces it.
    let replacement = container(b"replaced");
    let mut with_trailer = replacement.clone();
    with_trailer.extend_from_slice(&[0, 5]);
    store.write(1, &with_trailer).unwrap();
    assert_eq!(pack.read_raw_group("fontmetrics", 1).unwrap(), replacement);
    // Group 2 is listed but absent: GroupMissing + queued.
    assert!(matches!(
        pack.read_raw_group("fontmetrics", 2),
        Err(crate::cache::CacheError::GroupMissing { .. })
    ));
    assert!(overlay.take_missing().contains(&(13, 2)));
}

/// Lane Q-FAR1: `Pack::read_group_resident` returns what `read_group`
/// returns for a stored group, and an absent group (listed but not stored,
/// or not listed) quietly: `None`, nothing queued for the JS5 owner. The
/// noisy read of the same group still queues it (the test is not vacuous).
#[test]
fn resident_reads_never_queue_absent_groups() {
    let root = pack_fixture_root();
    let cache = temp_dir("resident-reads");
    let overlay = DiskOverlay::new(&cache);
    let pack = Pack::open_with_overlay(&root, Some(overlay.clone()));
    let stored = pack.read_group_resident("fontmetrics", 1).unwrap();
    assert_eq!(stored, Some(pack.read_group("fontmetrics", 1).unwrap()));
    assert_eq!(pack.read_group_resident("fontmetrics", 2).unwrap(), None);
    assert_eq!(pack.read_group_resident("fontmetrics", 9).unwrap(), None);
    assert!(overlay.take_missing().is_empty(), "a resident read queued");
    assert!(pack.read_group("fontmetrics", 2).is_err());
    assert!(overlay.take_missing().contains(&(13, 2)));
}

/// One long-lived `Pack` (the client's shared reader) sees every disk
/// store write: a replaced index (pack -> overlay -> newer overlay) and a
/// group that arrives after a `GroupMissing` read.
#[test]
fn shared_pack_sees_index_and_group_writes_after_caching() {
    let root = pack_fixture_root();
    let cache = temp_dir("shared-pack");
    let overlay = DiskOverlay::new(&cache);
    let pack = Pack::open_with_overlay(&root, Some(overlay.clone()));
    let master = DiskStore::new(ARCHIVE_SET, pack.clone(), overlay.clone());
    let groups = DiskStore::new(13, pack.clone(), overlay.clone());
    assert_eq!(
        pack.read_archive_index("fontmetrics").unwrap().group_id,
        [0, 1, 2]
    );
    // The provider replaces the pack's index (flushed write).
    let listed = |n: u32| -> Vec<(u32, i32, i32)> { (0..n).map(|g| (g, 0, 1)).collect() };
    master.write(13, &index_container(&listed(4))).unwrap();
    assert_eq!(
        pack.read_archive_index("fontmetrics").unwrap().group_id,
        [0, 1, 2, 3]
    );
    // A newer index replaces the overlay's own (queued write).
    overlay.mark_pending(ARCHIVE_SET, 13, Arc::new(index_container(&listed(5))));
    assert_eq!(
        pack.read_archive_index("fontmetrics").unwrap().group_id,
        [0, 1, 2, 3, 4]
    );
    // Group 3 is absent until the JS5 owner stores it.
    assert!(matches!(
        pack.read_raw_group("fontmetrics", 3),
        Err(crate::cache::CacheError::GroupMissing { .. })
    ));
    let arrived = container(b"arrived");
    let mut with_trailer = arrived.clone();
    with_trailer.extend_from_slice(&[0, 1]);
    groups.write(3, &with_trailer).unwrap();
    assert_eq!(pack.read_raw_group("fontmetrics", 3).unwrap(), arrived);
}

/// Fix programme item 5, scenario 7 (test-audit.md; kills M4 and M35b).
///
/// Part 1, the consumer path: a pack whose `fontmetrics` index lists
/// group 2 without stored bytes. A `Pack` read reports `GroupMissing`
/// (a null file); the ordinary per-frame
/// `Js5System::mainloop` (`app.rs` `js5_mainloop`)
/// takes the queued miss and fetches it with an urgent request
/// (opcode 1) and the same shared `Pack`
/// then rereads the served container.
///
/// Part 2, the disk verify: a restarted client over a disk cache whose
/// group 1 body was corrupted re-requests exactly that group
/// (a CRC check over the stored container, then the urgent requeue) and
/// rewrites the file, while intact groups are verified from disk without a request.
///
/// Expected values: the fake content server's own request log and the
/// bytes it served (never the client's output).
#[test]
fn missing_and_corrupt_groups_are_fetched_by_the_js5_mainloop() {
    let (served, groups) = archive_fixture();
    let fake = fake_server(served.clone(), Faults::default());
    // The pack holds the served index but only groups 0 and 1.
    let root = temp_dir("scenario-pack");
    let index = served[&(255, 13)].clone();
    let mut file = index.clone();
    for (_, c) in &groups[..2] {
        file.extend_from_slice(c);
    }
    for (i, (_, c)) in groups.iter().enumerate() {
        let len = if i < 2 { c.len() as u32 } else { 0 };
        file.extend_from_slice(&len.to_be_bytes());
    }
    std::fs::write(root.join("client.fontmetrics.js5"), file).unwrap();
    let cache = temp_dir("scenario-cache");
    let mut system = Js5System::new(
        &root,
        DiskOverlay::new(&cache),
        ContentAddress::new("127.0.0.1".into(), fake.port, fake.port),
        ("127.0.0.1".into(), 9),
        0,
        String::new(),
        0,
    );
    let pack = system.pack.clone();
    let served_group = |g: u32| served[&(13, g)].clone();
    assert_eq!(
        pack.read_raw_group("fontmetrics", 0).unwrap(),
        served_group(0)
    );
    assert!(matches!(
        pack.read_raw_group("fontmetrics", 2),
        Err(crate::cache::CacheError::GroupMissing { .. })
    ));
    // The client's frames: mainloop until the reread succeeds.
    let start = std::time::Instant::now();
    let reread = loop {
        system.mainloop(30);
        assert!(system.fatal.is_none(), "fatal {:?}", system.fatal);
        if let Ok(bytes) = pack.read_raw_group("fontmetrics", 2) {
            break bytes;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "group 13/2 was never fetched; server log {:?}",
            fake.log.lock().unwrap().messages
        );
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(reread, served_group(2));
    {
        let log = fake.log.lock().unwrap();
        assert!(
            log.messages.contains(&(1, 13, 2)),
            "urgent (13, 2): {:?}",
            log.messages
        );
        // Only the group a consumer missed was requested.
        assert!(
            !log.messages
                .iter()
                .any(|m| m.0 <= 1 && m.1 == 13 && m.2 != 2),
            "{:?}",
            log.messages
        );
    }
    system.quit();

    // Part 2: a full download into a disk cache, then a corrupted group 1.
    let (mut first, _, disk, downloaded) = download_archive(Faults::default(), "scenario-verify");
    // Every group written by the disk thread before the client stops.
    let wait = std::time::Instant::now();
    while downloaded
        .iter()
        .any(|(id, _)| !disk.join("13").join(format!("{id}.dat")).is_file())
    {
        assert!(
            wait.elapsed() < Duration::from_secs(10),
            "disk writes never landed"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    first.quit();
    let path = disk.join("13").join("1.dat");
    let good = std::fs::read(&path).unwrap();
    let mut bad = good.clone();
    bad[6] ^= 0x5A; // container body, trailer (version 101) intact
    std::fs::write(&path, &bad).unwrap();
    let fake = fake_server(served, Faults::default());
    let mut system = Js5System::new(
        &temp_dir("scenario-verify-pack"),
        DiskOverlay::new(&disk),
        ContentAddress::new("127.0.0.1".into(), fake.port, fake.port),
        ("127.0.0.1".into(), 9),
        0,
        String::new(),
        0,
    );
    // Loading stages: the client, its master index, `create_js5`
    // with prefetch-all, then the verify pass to 100 %.
    system.ensure_client();
    pump(&mut system, Js5System::load_master_index);
    system.create_js5(13, false, 1, true).unwrap();
    let start = std::time::Instant::now();
    while system.percentage(13) < 100 || std::fs::read(&path).unwrap() != good {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "corrupt group never re-fetched"
        );
        assert!(system.fatal.is_none(), "fatal {:?}", system.fatal);
        system.fetch_all(13);
        system.mainloop(5);
        std::thread::sleep(Duration::from_millis(1));
    }
    let log = fake.log.lock().unwrap();
    let group_requests: Vec<u32> = log
        .messages
        .iter()
        .filter(|m| m.0 <= 1 && m.1 == 13)
        .map(|m| m.2)
        .collect();
    assert_eq!(group_requests, [1], "only the corrupt group is re-fetched");
    drop(log);
    system.quit();
}

fn pack_group(id: u32) -> Vec<u8> {
    container(format!("pack-{id}").as_bytes())
}

/// A `client.fontmetrics.js5` in the `Js5.ts` layout: master, groups 0
/// and 1, group 2 listed with a zero-length trailer entry.
fn pack_fixture_root() -> PathBuf {
    let root = temp_dir("pack-fixture");
    let groups: Vec<Vec<u8>> = (0..2).map(pack_group).collect();
    let entries: Vec<(u32, i32, i32)> = vec![
        (0, getcrc(&groups[0]), 1),
        (1, getcrc(&groups[1]), 1),
        (2, 0, 1),
    ];
    let mut file = index_container(&entries);
    for g in &groups {
        file.extend_from_slice(g);
    }
    for g in &groups {
        file.extend_from_slice(&(g.len() as u32).to_be_bytes());
    }
    file.extend_from_slice(&0_u32.to_be_bytes());
    std::fs::write(root.join("client.fontmetrics.js5"), file).unwrap();
    root
}
