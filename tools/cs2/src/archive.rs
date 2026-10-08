//! Acquire an exact-profile donor independently of target runtime packs.
use crate::profile::{Book, Build, digest};
use anyhow::{Context, Result, ensure};
use native910::js5::{ArchiveIndex, decompress};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Write,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};

const API_ROOT: &str = "https://archive.openrs2.org";
const INDEX_ARCHIVE: u32 = 255;
const MANIFEST_FORMAT: u32 = 1;
const MANIFEST_FILE: &str = "cs2-cache.json";
pub const DEFAULT_WORKERS: usize = 8;
const MAX_WORKERS: usize = 32;
const REQUEST_TIMEOUT_SECONDS: u64 = 60;
const REQUEST_ATTEMPTS: usize = 4;
const MAX_RESPONSE_BYTES: u64 = 64 << 20;
const PROGRESS_GROUPS: usize = 2000;

pub struct FetchOptions<'a> {
    pub cache_id: Option<u64>,
    /// Additional archives; the profile's script archive is always included.
    pub archives: &'a [u32],
    pub output: &'a Path,
    pub workers: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct CacheMetadata {
    pub id: u64,
    pub scope: String,
    pub game: String,
    pub environment: String,
    pub language: String,
    pub builds: Vec<ArchivedBuild>,
    pub timestamp: Option<String>,
    pub valid_indexes: u64,
    pub indexes: u64,
    pub valid_groups: u64,
    pub groups: u64,
}

/// Archive metadata can omit either build field. An unknown minor never
/// becomes zero and never authorizes using an exact client profile.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ArchivedBuild {
    pub major: Option<u32>,
    pub minor: Option<u32>,
}

impl From<Build> for ArchivedBuild {
    fn from(build: Build) -> Self {
        Self {
            major: Some(build.major),
            minor: Some(build.minor),
        }
    }
}

impl std::fmt::Display for ArchivedBuild {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let field =
            |value: Option<u32>| value.map_or_else(|| "?".into(), |value| value.to_string());
        write!(formatter, "{}.{}", field(self.major), field(self.minor))
    }
}

#[derive(Deserialize, Serialize)]
pub struct ArchiveReceipt {
    pub index_sha256: String,
    pub groups: usize,
    pub files: usize,
    pub complete: bool,
}

#[derive(Deserialize, Serialize)]
pub struct Manifest {
    pub format: u32,
    pub source: String,
    pub cache: CacheMetadata,
    pub source_build: Build,
    pub source_client_md5: String,
    pub source_profile_sha256: String,
    pub highest_archived_build: ArchivedBuild,
    pub archives: BTreeMap<u32, ArchiveReceipt>,
}

#[derive(Serialize)]
pub struct FetchReport {
    pub manifest: Manifest,
    pub downloaded_groups: usize,
    pub reused_groups: usize,
}

/// Connection pooling is shared by the bounded workers. JS5 compression is
/// carried inside the bytes; HTTP content decoding is disabled in this client.
pub fn fetch(book: &Book, options: FetchOptions<'_>) -> Result<FetchReport> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(REQUEST_TIMEOUT_SECONDS)))
        .https_only(true)
        .max_idle_connections_per_host(MAX_WORKERS)
        .build()
        .new_agent();
    let download = |url: &str| -> Result<Vec<u8>> {
        let mut last_error = None;
        for attempt in 0..REQUEST_ATTEMPTS {
            let result = (|| {
                let mut response = agent
                    .get(url)
                    .header("User-Agent", "Alto CS2 donor tooling")
                    .call()?;
                ensure!(response.status().is_success(), "HTTP {}", response.status());
                Ok(response
                    .body_mut()
                    .with_config()
                    .limit(MAX_RESPONSE_BYTES)
                    .read_to_vec()?)
            })();
            match result {
                Ok(bytes) => return Ok(bytes),
                Err(error) => last_error = Some(error),
            }
            if attempt + 1 < REQUEST_ATTEMPTS {
                std::thread::sleep(Duration::from_secs(u64::try_from(attempt + 1)?));
            }
        }
        Err(last_error.context("no archive request attempt")?)
            .with_context(|| format!("download {url}"))
    };
    fetch_with(book, options, &download)
}

fn compatible(cache: &CacheMetadata, build: Build) -> bool {
    cache.scope == "runescape"
        && cache.game == "runescape"
        && cache.environment == "live"
        && cache.language == "en"
        && cache.builds.iter().any(|candidate| {
            candidate.major == Some(build.major) && candidate.minor == Some(build.minor)
        })
        && cache.indexes > 0
        && cache.indexes == cache.valid_indexes
        && cache.groups == cache.valid_groups
}

fn fetch_with(
    book: &Book,
    options: FetchOptions<'_>,
    download: &(impl Fn(&str) -> Result<Vec<u8>> + Sync),
) -> Result<FetchReport> {
    ensure!(
        (1..=MAX_WORKERS).contains(&options.workers),
        "worker count must be between 1 and {MAX_WORKERS}"
    );
    let caches: Vec<CacheMetadata> =
        serde_json::from_slice(&download(&format!("{API_ROOT}/caches.json"))?)?;
    let highest_archived_build = caches
        .iter()
        .filter(|cache| {
            cache.scope == "runescape"
                && cache.game == "runescape"
                && cache.environment == "live"
                && cache.language == "en"
                && cache.indexes > 0
                && cache.indexes == cache.valid_indexes
                && cache.groups == cache.valid_groups
        })
        .flat_map(|cache| cache.builds.iter())
        .filter(|build| build.major.is_some())
        .max_by_key(|build| (build.major, build.minor))
        .copied()
        .context("archive has no complete English live RuneScape build")?;
    let cache = caches
        .into_iter()
        .filter(|cache| compatible(cache, book.profile.build))
        .filter(|cache| options.cache_id.is_none_or(|id| cache.id == id))
        .max_by(|left, right| (&left.timestamp, left.id).cmp(&(&right.timestamp, right.id)))
        .context("no complete English live cache matches the exact profile build")?;
    eprintln!(
        "selected cache {} for {}; highest archived build is {}",
        cache.id, book.profile.build, highest_archived_build
    );
    let source = format!("{API_ROOT}/caches/{}/{}", cache.scope, cache.id);
    std::fs::create_dir_all(options.output)?;
    let cache_lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(options.output.join(".cs2-cache.lock"))?;
    cache_lock
        .try_lock()
        .context("another fetch owns this output directory")?;
    let manifest_path = options.output.join(MANIFEST_FILE);
    let mut manifest = if manifest_path.exists() {
        let previous: Manifest = serde_json::from_slice(&std::fs::read(&manifest_path)?)?;
        ensure!(
            previous.format == MANIFEST_FORMAT
                && previous.source == source
                && previous.cache.id == cache.id
                && previous.source_build == book.profile.build
                && previous.source_client_md5 == book.profile.client_md5,
            "output belongs to another cache or client profile; select a separate output"
        );
        previous
    } else {
        Manifest {
            format: MANIFEST_FORMAT,
            source: source.clone(),
            cache: cache.clone(),
            source_build: book.profile.build,
            source_client_md5: book.profile.client_md5.clone(),
            source_profile_sha256: book.sha256.clone(),
            highest_archived_build,
            archives: BTreeMap::new(),
        }
    };
    let mut archives: BTreeSet<_> = options.archives.iter().copied().collect();
    archives.insert(book.profile.script_archive);
    ensure!(
        archives.iter().all(|archive| *archive < INDEX_ARCHIVE),
        "request data archives; index archive is fetched automatically"
    );
    let mut indexes = BTreeMap::new();
    let mut jobs = Vec::new();
    for archive in &archives {
        let bytes = download(&format!(
            "{source}/archives/{INDEX_ARCHIVE}/groups/{archive}.dat"
        ))?;
        let sha256 = digest(&bytes);
        if *archive == book.profile.script_archive {
            ensure!(
                sha256 == book.profile.script_index_sha256,
                "selected cache script index differs from the profile; recover an exact profile or explicitly select its matching cache"
            );
        }
        if let Some(previous) = manifest.archives.get(archive) {
            ensure!(
                previous.index_sha256 == sha256,
                "archive {archive} index changed since the cache was pinned"
            );
        }
        let path = options
            .output
            .join(INDEX_ARCHIVE.to_string())
            .join(format!("{archive}.dat"));
        if path.exists() {
            ensure!(
                digest(&std::fs::read(&path)?) == sha256,
                "existing archive {archive} index belongs to different cache bytes"
            );
        }
        let index = ArchiveIndex::decode(&decompress(&bytes)?)?;
        let mut files = 0;
        for group in &index.group_id {
            files += index.file_count_for_group(*group)?;
            jobs.push(GroupJob {
                archive: *archive,
                group: *group,
                version: index.group_versions[*group as usize],
                checksum: index.group_checksums[*group as usize],
            });
        }
        manifest.archives.insert(
            *archive,
            ArchiveReceipt {
                index_sha256: sha256,
                groups: index.group_count,
                files,
                complete: false,
            },
        );
        indexes.insert(path, bytes);
    }
    // All selected identities are checked before any local cache is changed.
    manifest.source_profile_sha256 = book.sha256.clone();
    manifest.highest_archived_build = highest_archived_build;
    manifest.cache = cache;
    std::fs::create_dir_all(options.output)?;
    atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    for (path, bytes) in indexes {
        atomic_write(&path, &bytes)?;
    }
    for archive in &archives {
        std::fs::create_dir_all(options.output.join(archive.to_string()))?;
    }
    let cursor = AtomicUsize::new(0);
    let stopped = AtomicBool::new(false);
    let downloaded = AtomicUsize::new(0);
    let reused = AtomicUsize::new(0);
    std::thread::scope(|scope| -> Result<()> {
        let handles: Vec<_> = (0..options.workers)
            .map(|_| {
                scope.spawn(|| -> Result<()> {
                    while !stopped.load(Ordering::Relaxed) {
                        let position = cursor.fetch_add(1, Ordering::Relaxed);
                        let Some(job) = jobs.get(position) else { break };
                        match job.fetch(options.output, download) {
                            Ok(true) => {
                                downloaded.fetch_add(1, Ordering::Relaxed);
                            }
                            Ok(false) => {
                                reused.fetch_add(1, Ordering::Relaxed);
                            }
                            Err(error) => {
                                stopped.store(true, Ordering::Relaxed);
                                return Err(error);
                            }
                        }
                        let verified =
                            downloaded.load(Ordering::Relaxed) + reused.load(Ordering::Relaxed);
                        if verified.is_multiple_of(PROGRESS_GROUPS) {
                            eprintln!("verified {verified}/{} groups", jobs.len());
                        }
                    }
                    Ok(())
                })
            })
            .collect();
        let mut failure = None;
        for handle in handles {
            let result = handle
                .join()
                .map_err(|_| anyhow::anyhow!("archive worker panicked"))?;
            if let Err(error) = result {
                failure.get_or_insert(error);
            }
        }
        if let Some(error) = failure {
            return Err(error);
        }
        Ok(())
    })?;
    for archive in archives {
        manifest
            .archives
            .get_mut(&archive)
            .context("missing archive receipt")?
            .complete = true;
    }
    atomic_write(&manifest_path, &serde_json::to_vec_pretty(&manifest)?)?;
    Ok(FetchReport {
        manifest,
        downloaded_groups: downloaded.load(Ordering::Relaxed),
        reused_groups: reused.load(Ordering::Relaxed),
    })
}

struct GroupJob {
    archive: u32,
    group: u32,
    version: i32,
    checksum: i32,
}

impl GroupJob {
    fn fetch(
        &self,
        root: &Path,
        download: &(impl Fn(&str) -> Result<Vec<u8>> + Sync),
    ) -> Result<bool> {
        let path = root
            .join(self.archive.to_string())
            .join(format!("{}.dat", self.group));
        let checksum = self.checksum as u32;
        if path.exists() && crc32fast::hash(&std::fs::read(&path)?) == checksum {
            return Ok(false);
        }
        let url = format!(
            "{API_ROOT}/caches/runescape/archives/{}/groups/{}/versions/{}/checksums/{}.dat",
            self.archive, self.group, self.version, self.checksum
        );
        let bytes = download(&url)?;
        ensure!(
            crc32fast::hash(&bytes) == checksum,
            "checksum mismatch in archive {} group {}",
            self.archive,
            self.group
        );
        atomic_write(&path, &bytes)?;
        Ok(true)
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache file has no parent")?;
    std::fs::create_dir_all(parent)?;
    let temporary = path.with_extension(format!(
        "{}-{}.tmp",
        path.extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("file"),
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)?;
        Ok(())
    })();
    drop(file);
    if temporary.exists() {
        std::fs::remove_file(&temporary)?;
    }
    result
}

#[cfg(test)]
pub(crate) fn verify_fetching(original: &Book) {
    use native910::packet::ByteWriter;
    use std::sync::{Mutex, atomic::AtomicBool};
    const INDEX_PROTOCOL: u8 = 6;
    const INDEX_VERSION: i32 = 1;
    const UNNAMED_FLAGS: u8 = 0;
    const UNCOMPRESSED: u8 = 0;
    const GROUP_VERSION: i32 = 7;
    const FIRST_GROUP: u16 = 3;
    const SECOND_GROUP: u16 = 9;
    const FIRST_FILE: u16 = 0;
    const FILES_PER_GROUP: u16 = 1;
    const GROUP_COUNT: u16 = 2;
    const OLD_CACHE_ID: u64 = 101;
    const LATEST_CACHE_ID: u64 = 102;
    const SINGLE_WORKER: usize = 1;
    let container = |payload: &[u8]| {
        let mut out = ByteWriter::default();
        out.p1(UNCOMPRESSED);
        out.p4s(i32::try_from(payload.len()).unwrap());
        out.data.extend_from_slice(payload);
        out.data
    };
    let groups = [container(b"first"), container(b"second")];
    let mut payload = ByteWriter::default();
    payload.p1(INDEX_PROTOCOL);
    payload.p4s(INDEX_VERSION);
    payload.p1(UNNAMED_FLAGS);
    payload.p2(GROUP_COUNT);
    payload.p2(FIRST_GROUP);
    payload.p2(SECOND_GROUP - FIRST_GROUP);
    for bytes in &groups {
        payload.p4s(crc32fast::hash(bytes) as i32);
    }
    for _ in &groups {
        payload.p4s(GROUP_VERSION);
    }
    for _ in &groups {
        payload.p2(FILES_PER_GROUP);
    }
    for _ in &groups {
        payload.p2(FIRST_FILE);
    }
    let index = container(&payload.data);
    let mut profile = original.profile.clone();
    profile.script_index_sha256 = digest(&index);
    let book = Book::parse(&serde_json::to_vec(&profile).unwrap()).unwrap();
    let archive = book.profile.script_archive;
    let cache = |id, timestamp: &str| CacheMetadata {
        id,
        scope: "runescape".into(),
        game: "runescape".into(),
        environment: "live".into(),
        language: "en".into(),
        builds: vec![book.profile.build.into()],
        timestamp: Some(timestamp.into()),
        valid_indexes: 1,
        indexes: 1,
        valid_groups: u64::from(GROUP_COUNT),
        groups: u64::from(GROUP_COUNT),
    };
    let mut unknown = cache(LATEST_CACHE_ID + 1, "2000-03-01T00:00:00Z");
    unknown.builds = vec![ArchivedBuild {
        major: Some(book.profile.build.major + 1),
        minor: None,
    }];
    let metadata = serde_json::to_vec(&[
        cache(OLD_CACHE_ID, "2000-01-01T00:00:00Z"),
        cache(LATEST_CACHE_ID, "2000-02-01T00:00:00Z"),
        unknown,
    ])
    .unwrap();
    let metadata_url = format!("{API_ROOT}/caches.json");
    let index_url = format!(
        "{API_ROOT}/caches/runescape/{LATEST_CACHE_ID}/archives/{INDEX_ARCHIVE}/groups/{archive}.dat"
    );
    let jobs = [FIRST_GROUP, SECOND_GROUP].into_iter().zip(&groups)
        .map(|(group, bytes)| {
            let job = GroupJob { archive, group: u32::from(group), version: GROUP_VERSION, checksum: crc32fast::hash(bytes) as i32 };
            let url = format!("{API_ROOT}/caches/runescape/archives/{archive}/groups/{group}/versions/{GROUP_VERSION}/checksums/{}.dat", job.checksum);
            (job, url)
        }).collect::<Vec<_>>();
    let failed = AtomicBool::new(true);
    let invalid_body = AtomicBool::new(false);
    let wrong_index = AtomicBool::new(false);
    let requested = Mutex::new(Vec::new());
    let download = |url: &str| -> Result<Vec<u8>> {
        requested.lock().unwrap().push(url.to_owned());
        if url == metadata_url {
            return Ok(metadata.clone());
        }
        if url == index_url {
            return Ok(if wrong_index.load(Ordering::Relaxed) {
                container(b"wrong index")
            } else {
                index.clone()
            });
        }
        for (position, (_, group_url)) in jobs.iter().enumerate() {
            if url == group_url {
                if position == 1 && failed.load(Ordering::Relaxed) {
                    anyhow::bail!("recorded disconnect");
                }
                if invalid_body.load(Ordering::Relaxed) {
                    return Ok(container(b"wrong group"));
                }
                return Ok(groups[position].clone());
            }
        }
        anyhow::bail!("unexpected fixture request: {url}")
    };
    let directory = std::env::temp_dir().join(format!("alto-cs2-fetch-{}", std::process::id()));
    std::fs::create_dir(&directory).unwrap();
    let options = || FetchOptions {
        cache_id: None,
        archives: &[],
        output: &directory,
        workers: SINGLE_WORKER,
    };
    assert!(fetch_with(&book, options(), &download).is_err());
    let first_path = directory
        .join(archive.to_string())
        .join(format!("{FIRST_GROUP}.dat"));
    let second_path = directory
        .join(archive.to_string())
        .join(format!("{SECOND_GROUP}.dat"));
    assert_eq!(std::fs::read(&first_path).unwrap(), groups[0]);
    assert!(!second_path.exists());
    let receipt = || {
        serde_json::from_slice::<Manifest>(&std::fs::read(directory.join(MANIFEST_FILE)).unwrap())
            .unwrap()
    };
    assert!(!receipt().archives[&archive].complete);
    std::fs::write(&first_path, b"corrupt").unwrap();
    failed.store(false, Ordering::Relaxed);
    invalid_body.store(true, Ordering::Relaxed);
    assert!(fetch_with(&book, options(), &download).is_err());
    assert_eq!(std::fs::read(&first_path).unwrap(), b"corrupt");
    invalid_body.store(false, Ordering::Relaxed);
    let completed = fetch_with(&book, options(), &download).unwrap();
    assert_eq!(completed.manifest.cache.id, LATEST_CACHE_ID);
    assert_eq!(completed.manifest.highest_archived_build.minor, None);
    assert_eq!(completed.downloaded_groups, usize::from(GROUP_COUNT));
    assert_eq!(completed.reused_groups, 0);
    assert!(receipt().archives[&archive].complete);
    assert_eq!(std::fs::read(&first_path).unwrap(), groups[0]);
    assert_eq!(std::fs::read(&second_path).unwrap(), groups[1]);
    requested.lock().unwrap().clear();
    let resumed = fetch_with(&book, options(), &download).unwrap();
    assert_eq!(resumed.downloaded_groups, 0);
    assert_eq!(resumed.reused_groups, usize::from(GROUP_COUNT));
    assert_eq!(
        &*requested.lock().unwrap(),
        &[metadata_url.clone(), index_url.clone()]
    );
    let original_receipt = std::fs::read(directory.join(MANIFEST_FILE)).unwrap();
    wrong_index.store(true, Ordering::Relaxed);
    assert!(fetch_with(&book, options(), &download).is_err());
    assert_eq!(
        std::fs::read(directory.join(MANIFEST_FILE)).unwrap(),
        original_receipt
    );
    wrong_index.store(false, Ordering::Relaxed);
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(directory.join(".cs2-cache.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(fetch_with(&book, options(), &download).is_err());
    drop(lock);
    std::fs::remove_dir_all(&directory).unwrap();
}
