//! Sound sources and sounds: the loader that reads sound groups from the
//! cache off the client thread, the two kinds of stream (a chunked song, a
//! whole effect group) and [`Sound`], one playing request with its life
//! cycle, volume fades, delay and voices.
//!
//! Decoded PCM is never cached: the effect cache holds the compressed
//! groups, and every voice decodes from them.

use crate::audio_adjust::{AdjustContext, Adjuster};
use crate::audio_backend::{self as backend, lock};
use crate::audio_voice::{ChunkFetcher, Source, VoiceRequest, VoiceState};
use anyhow::{bail, ensure, Result};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Arc, Condvar, Mutex};

/// The two cache archives the audio stack reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Archive {
    Vorbis,
    AudioStreams,
}

impl Archive {
    fn name(self) -> &'static str {
        match self {
            Self::Vorbis => "vorbis",
            Self::AudioStreams => "audiostreams",
        }
    }
}

#[derive(Default)]
struct LoaderState {
    ready: HashMap<(Archive, i32), Arc<[u8]>>,
    queue: VecDeque<(Archive, i32)>,
    requested: HashSet<(Archive, i32)>,
    failed: HashSet<(Archive, i32)>,
    stop: bool,
}

/// Where [`SoundLoader`] reads its groups (the client implements it for the
/// cache), so the audio crate does not depend on the cache crate. Errors
/// carry the source's own text.
pub trait GroupSource: Send + Sync {
    /// The files of group `group_id` of the archive named `archive_name`.
    fn read_group(
        &self,
        archive_name: &str,
        group_id: u32,
    ) -> anyhow::Result<BTreeMap<u32, Vec<u8>>>;
}

/// Reads the single file of a sound group. A group that is not read yet
/// answers `None`; a loader thread reads groups off the client thread, and a
/// group that fails to read stays `None` forever.
pub struct SoundLoader {
    pack: Option<Box<dyn GroupSource>>,
    state: Mutex<LoaderState>,
    wake: Condvar,
    synchronous: bool,
}

impl SoundLoader {
    /// A loader backed by `pack`; `synchronous` reads inline (tests).
    pub fn new<S: GroupSource + 'static>(pack: Option<S>, synchronous: bool) -> Arc<Self> {
        let loader = Arc::new(Self {
            pack: pack.map(|pack| Box::new(pack) as Box<dyn GroupSource>),
            state: Mutex::new(LoaderState::default()),
            wake: Condvar::new(),
            synchronous,
        });
        if !synchronous && loader.pack.is_some() {
            let worker = loader.clone();
            let spawned = std::thread::Builder::new()
                .name("client910-audio-loader".into())
                .spawn(move || worker.run());
            if let Err(error) = spawned {
                log::warn!("[client910] audio loader unavailable: {error}");
            }
        }
        loader
    }

    fn read(&self, archive: Archive, id: i32) -> Result<Arc<[u8]>> {
        let Some(pack) = &self.pack else {
            bail!("no cache pack");
        };
        let mut files = pack.read_group(archive.name(), id as u32)?;
        ensure!(
            files.len() == 1,
            "sound group {id} holds {} files, expected one",
            files.len()
        );
        match files.remove(&0) {
            Some(bytes) => Ok(bytes.into()),
            None => bail!("sound group {id} has no file 0"),
        }
    }

    fn run(self: Arc<Self>) {
        loop {
            let key = {
                let mut state = lock(&self.state);
                loop {
                    if state.stop {
                        return;
                    }
                    if let Some(key) = state.queue.pop_front() {
                        break key;
                    }
                    state = self.wake.wait(state).unwrap_or_else(|e| e.into_inner());
                }
            };
            let result = self.read(key.0, key.1);
            let mut state = lock(&self.state);
            match result {
                Ok(bytes) => {
                    state.ready.insert(key, bytes);
                }
                Err(error) => {
                    log::warn!(
                        "[client910] audio {} group {} unavailable: {error:#}",
                        key.0.name(),
                        key.1
                    );
                    state.failed.insert(key);
                }
            }
            state.requested.remove(&key);
        }
    }

    /// The single file of group `id`, or `None` while it is still loading
    /// (or permanently when invalid).
    pub fn fetch_file(&self, archive: Archive, id: i32) -> Option<Arc<[u8]>> {
        if id < 0 {
            return None;
        }
        let key = (archive, id);
        if self.synchronous {
            return match self.read(archive, id) {
                Ok(bytes) => Some(bytes),
                Err(error) => {
                    let mut state = lock(&self.state);
                    if state.failed.insert(key) {
                        log::warn!(
                            "[client910] audio {} group {id} unavailable: {error:#}",
                            archive.name()
                        );
                    }
                    None
                }
            };
        }
        let mut state = lock(&self.state);
        if let Some(bytes) = state.ready.remove(&key) {
            return Some(bytes);
        }
        if self.pack.is_some() && !state.failed.contains(&key) && state.requested.insert(key) {
            state.queue.push_back(key);
            self.wake.notify_one();
        }
        None
    }
}

impl Drop for SoundLoader {
    fn drop(&mut self) {
        lock(&self.state).stop = true;
        self.wake.notify_all();
    }
}

/// Stop the loader thread (it holds its own `Arc`).
pub fn stop_loader(loader: &SoundLoader) {
    lock(&loader.state).stop = true;
    loader.wake.notify_all();
}

/// Whether a stream's data is available.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamState {
    Unset,
    /// Waiting for its group to load.
    Pending,
    Loaded,
}

/// The data a sound plays from: a chunked song (`song`, read chunk by
/// chunk from the `audiostreams` archive as the decoder asks) or a whole
/// effect group (read once from the `vorbis` archive).
pub struct Stream {
    pub song: bool,
    /// The archive group id.
    pub id: i32,
    state: StreamState,
    /// The loaded group of an effect.
    bytes: Option<Arc<[u8]>>,
    js5: Arc<SoundLoader>,
}

pub type StreamRef = std::rc::Rc<std::cell::RefCell<Stream>>;

impl Stream {
    pub fn new(song: bool, id: i32, js5: Arc<SoundLoader>) -> Self {
        Self {
            song,
            id,
            state: StreamState::Pending,
            bytes: None,
            js5,
        }
    }

    /// Poll the load. Returns the stream's cache weight (the group's byte
    /// size; 0 for a song) when it has just finished loading.
    pub fn load(&mut self) -> Option<i32> {
        if self.state != StreamState::Pending {
            return None;
        }
        if self.song {
            self.state = StreamState::Loaded;
            return Some(0);
        }
        let bytes = self.js5.fetch_file(Archive::Vorbis, self.id)?;
        let len = bytes.len() as i32;
        self.bytes = Some(bytes);
        self.state = StreamState::Loaded;
        Some(len)
    }

    pub fn state(&self) -> StreamState {
        self.state
    }

    /// The whole group of a loaded effect.
    pub fn bytes(&self) -> Option<Arc<[u8]>> {
        self.bytes.clone()
    }

    /// The chunk fetcher of a song; chunk group `0` means the song's own
    /// header group.
    pub fn chunk_fetcher(&self) -> ChunkFetcher {
        let js5 = self.js5.clone();
        let id = self.id;
        Arc::new(move |group: i32| {
            let group = if group == 0 { id } else { group };
            js5.fetch_file(Archive::AudioStreams, group)
        })
    }
}

/// The life cycle of a [`Sound`], ordered so that comparisons read as "has
/// it reached this stage yet".
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SoundState {
    Unset = 0,
    /// Attached to a stream.
    Created = 1,
    /// Waiting for the stream to load.
    Starting = 2,
    /// Stream loaded, waiting for a voice.
    Loaded = 3,
    /// Playing on a voice.
    Playing = 4,
    FadingOut = 5,
    /// Stopped, or paused under a jingle.
    Stopped = 6,
    Finished = 7,
    Reserved = 8,
    /// Back in the API's sound pool.
    Released = 9,
}

/// What a sound is for; the API treats songs and jingles specially.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundType {
    Song,
    Jingle,
    /// A positional animation sound.
    SequenceArea,
    /// An animation sound of the local player.
    SequenceLocal,
    /// A `sound_*` effect requested by a script.
    Script,
    /// A server-requested effect or speech.
    Effect,
    /// A positioned location's background loop.
    PositionedLoop,
    /// A positioned location's random one-shot.
    PositionedRandom,
}

/// One playing request: a stream, how loud, where, and the voices it holds.
pub struct Sound {
    pub stream: Option<StreamRef>,
    pub state: SoundState,
    /// A start was requested.
    started: bool,
    voices: Vec<backend::VoiceRef>,
    /// The API owns the sound: it reaps it when it stops. Sounds owned by
    /// a caller (positioned sounds, cutscenes) stay until the caller hands
    /// them over.
    pub owned: bool,
    pub position: Option<[f32; 3]>,
    pub bus: i32,
    looping: bool,
    /// Extra loop passes (negative: forever).
    loop_count: i32,
    volume: f32,
    adjuster: Option<Adjuster>,
    pub size: f32,
    pub range: f32,
    /// Updates until the sound starts (-1: no delay), and whether it is held
    /// until its group is started.
    delay: i32,
    held: bool,
    pub kind: Option<SoundType>,
    /// A fade requested before a voice was available.
    fade_ms: i32,
    /// Playback rate relative to the file's own sample rate.
    rate: f32,
}

impl Sound {
    /// A new sound attached to `stream`. It starts with bus 0 and rate 0.0
    /// (a recycled sound is cleared to bus -1 and rate 1.0 instead); the API
    /// sets both before the sound starts.
    pub fn new(stream: StreamRef) -> Self {
        let mut sound = Self {
            stream: None,
            state: SoundState::Unset,
            started: false,
            voices: Vec::new(),
            owned: false,
            position: None,
            bus: 0,
            looping: false,
            loop_count: 0,
            volume: 0.0,
            adjuster: None,
            size: 0.0,
            range: 0.0,
            delay: 0,
            held: false,
            kind: None,
            fade_ms: 0,
            rate: 0.0,
        };
        sound.attach(stream);
        sound
    }

    /// Reset a sound for reuse.
    fn clear(&mut self) {
        self.stream = None;
        self.state = SoundState::Unset;
        self.started = false;
        self.voices.clear();
        self.owned = false;
        self.position = None;
        self.bus = -1;
        self.looping = false;
        self.loop_count = 0;
        self.volume = 0.0;
        self.adjuster = None;
        self.size = 0.0;
        self.range = 0.0;
        self.delay = 0;
        self.held = false;
        self.kind = None;
        self.fade_ms = 0;
        self.rate = 1.0;
    }

    /// Attach a stream to a fresh or recycled sound.
    pub fn attach(&mut self, stream: StreamRef) {
        self.stream = Some(stream);
        self.volume = 0.0;
        self.state = SoundState::Created;
    }

    /// Stop and return the sound to the pool.
    pub fn release(&mut self, key: usize, now: i64) {
        if self.state != SoundState::Stopped && self.state != SoundState::Finished {
            self.fade_out(key, 0, now);
        }
        for voice in &self.voices {
            let mut voice = lock(voice);
            if voice.owner() == Some(key) {
                voice.release();
            }
        }
        self.clear();
        self.state = SoundState::Released;
    }

    /// Load the stream without starting playback.
    pub fn preload(&mut self) {
        let parked = matches!(
            self.state,
            SoundState::Reserved | SoundState::Released | SoundState::Unset | SoundState::FadingOut
        );
        if !self.started
            && !parked
            && (self.state <= SoundState::Created || self.state >= SoundState::Stopped)
        {
            self.state = SoundState::Starting;
        }
    }

    /// Request playback.
    pub fn start(&mut self) {
        if matches!(
            self.state,
            SoundState::Reserved | SoundState::Released | SoundState::Unset | SoundState::FadingOut
        ) {
            return;
        }
        if matches!(self.state, SoundState::Starting | SoundState::Loaded) && !self.started {
            self.started = true;
        } else if self.state < SoundState::Starting || self.state >= SoundState::Stopped {
            self.state = SoundState::Starting;
            self.started = true;
        }
    }

    /// Fade the voices out over `ms` milliseconds (stop at once when it is
    /// not positive).
    pub fn fade_out(&mut self, key: usize, ms: i32, now: i64) {
        if self.state >= SoundState::Stopped {
            return;
        }
        if self.state < SoundState::Playing {
            self.state = SoundState::Stopped;
            self.started = false;
        } else if ms <= 0 {
            self.voices.retain(|voice| {
                let mut voice = lock(voice);
                if voice.owner() == Some(key) {
                    voice.stop();
                    true
                } else {
                    false
                }
            });
            self.state = SoundState::Stopped;
            self.started = false;
        } else {
            self.state = SoundState::FadingOut;
            self.voices.retain(|voice| {
                let mut voice = lock(voice);
                if voice.owner() == Some(key) {
                    voice.fade(0.0, ms, now);
                    true
                } else {
                    false
                }
            });
        }
    }

    /// Pause under a jingle.
    pub fn pause(&mut self, key: usize) {
        self.state = SoundState::Stopped;
        for voice in &self.voices {
            let mut voice = lock(voice);
            if voice.owner() == Some(key) {
                voice.pause();
            }
        }
    }

    /// Resume after a jingle.
    pub fn resume(&mut self, key: usize) {
        self.state = SoundState::Starting;
        for voice in &self.voices {
            let mut voice = lock(voice);
            if voice.owner() == Some(key) {
                voice.resume();
            }
        }
    }

    pub fn set_adjuster(&mut self, adjuster: Option<Adjuster>) {
        self.adjuster = adjuster;
    }

    /// Move a positioned sound: the sound and the voices it drives see the
    /// new position on their next adjuster pass.
    pub fn set_position(&mut self, key: usize, position: [f32; 3]) {
        if self.position.is_none() {
            return;
        }
        self.position = Some(position);
        if let Some(adjuster) = self.adjuster.as_mut() {
            adjuster.position = position;
        }
        for voice in &self.voices {
            let mut voice = lock(voice);
            if voice.owner() == Some(key) {
                voice.set_adjuster_position(position);
            }
        }
    }

    /// Claim a voice once the stream is loaded and reap finished voices.
    /// `touch` marks an effect as recently used in the effect cache.
    pub fn update(
        &mut self,
        key: usize,
        backend: &crate::audio_backend::SoundBackend,
        ctx: &AdjustContext,
        now: i64,
        touch: &mut dyn FnMut(i32),
    ) {
        let Some(stream) = self.stream.clone() else {
            return;
        };
        if self.state == SoundState::Starting && stream.borrow().state() == StreamState::Loaded {
            self.state = SoundState::Loaded;
        }
        if self.state == SoundState::Loaded && self.started {
            if let Some(voice_ref) = backend.allocate() {
                let mut voice = lock(&voice_ref);
                let source = {
                    let stream = stream.borrow();
                    if stream.song {
                        Some(Source::Streamed(stream.chunk_fetcher()))
                    } else {
                        touch(stream.id);
                        stream.bytes().map(Source::Group)
                    }
                };
                let volume = if self.fade_ms > 0 { 0.0 } else { self.volume };
                match voice.begin(VoiceRequest {
                    source,
                    bus: self.bus,
                    volume,
                    looping: self.looping,
                    loop_count: self.loop_count,
                    rate: self.rate,
                    owner: key,
                }) {
                    Ok(true) => {
                        self.state = SoundState::Playing;
                        voice.set_adjuster(self.adjuster, ctx);
                        voice.fade(self.volume, self.fade_ms, now);
                        voice.start_playing(&backend.buss, ctx);
                        drop(voice);
                        self.voices.push(voice_ref.clone());
                        self.started = false;
                    }
                    Ok(false) => {
                        if voice.state() == VoiceState::Finished {
                            self.state = SoundState::Finished;
                        }
                        voice.release();
                    }
                    Err(error) => {
                        let id = stream.borrow().id;
                        log::warn!("[client910] audio stream {id} rejected: {error:#}");
                        voice.release();
                        self.state = SoundState::Finished;
                        self.started = false;
                    }
                }
            }
        }
        let mut all_done = true;
        let fading = self.state == SoundState::FadingOut;
        self.voices.retain(|voice| {
            let mut voice = lock(voice);
            if voice.owner() != Some(key) {
                return false;
            }
            if fading {
                if voice.volume() == 0.0 {
                    voice.stop();
                } else {
                    all_done = false;
                }
            }
            if voice.is_finished() || voice.is_stopping() {
                voice.release();
                false
            } else {
                all_done = false;
                true
            }
        });
        if all_done && self.state >= SoundState::Playing && self.state < SoundState::Stopped {
            self.state = if self.state == SoundState::FadingOut {
                SoundState::Stopped
            } else {
                SoundState::Finished
            };
        }
    }

    pub fn set_bus(&mut self, bus: i32) {
        self.bus = bus;
    }

    /// Delay the start by `delay` updates; `held` keeps it until its group
    /// is started.
    pub fn set_delay(&mut self, delay: i32, held: bool) {
        self.held = held;
        self.delay = delay;
    }

    /// Let a held sound count down. Returns whether it has a delay left.
    pub fn release_hold(&mut self) -> bool {
        self.held = false;
        self.delay != 0
    }

    /// The per-update delay countdown; the sound starts when it reaches 0.
    pub fn tick_delay(&mut self) {
        if self.delay > -1 && !self.held {
            self.delay -= 1;
        }
        if self.delay == 0 {
            self.start();
        }
    }

    /// Fade to `volume` over `ms` milliseconds. Without a voice yet the fade
    /// length is remembered for the start.
    pub fn set_volume(&mut self, key: usize, volume: f32, ms: i32, now: i64) {
        self.volume = volume;
        let mut applied = 0;
        self.voices.retain(|voice| {
            let mut voice = lock(voice);
            if voice.owner() == Some(key) {
                voice.fade(volume, ms, now);
                applied += 1;
                true
            } else {
                false
            }
        });
        if applied == 0 {
            self.fade_ms = ms;
        }
    }

    pub fn set_looping(&mut self, looping: bool, count: i32) {
        self.looping = looping;
        self.loop_count = count;
    }

    /// Set the playback rate; a negative rate is ignored (NaN is accepted).
    pub fn set_rate(&mut self, rate: f32) {
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(rate < 0.0) {
            self.rate = rate;
        }
    }

    /// The id of the sound's stream.
    pub fn stream_id(&self) -> Option<i32> {
        self.stream.as_ref().map(|s| s.borrow().id)
    }
}
