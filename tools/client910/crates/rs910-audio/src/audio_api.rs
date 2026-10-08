//! The client's audio owner: the song and jingle state machine, sound groups,
//! the bus routing tree, the stream caches and the positional gain
//! adjusters.

use crate::audio_adjust::{length, sub};
use crate::audio_backend::{
    buss_type, sub_buss_type, Output, SoundBackend, VolumePreferences, VolumeProvider,
};
use crate::audio_stream::{Sound, SoundLoader, SoundState, SoundType, Stream, StreamRef};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub use crate::audio_adjust::{attenuation, AdjustContext, Adjuster, SoundShape};

/// A weighted least-recently-used cache of streams: a hit moves an entry to
/// the back, and inserting evicts from the front until the weights fit.
struct Lru {
    capacity: i32,
    remaining: i32,
    entries: Vec<(i32, StreamRef, i32)>,
}

impl Lru {
    fn new(capacity: i32) -> Self {
        Self {
            capacity,
            remaining: capacity,
            entries: Vec::new(),
        }
    }

    /// Look up a stream; a hit becomes the most recently used.
    fn get(&mut self, key: i32) -> Option<StreamRef> {
        let index = self.entries.iter().position(|e| e.0 == key)?;
        let entry = self.entries.remove(index);
        let value = entry.1.clone();
        self.entries.push(entry);
        Some(value)
    }

    fn remove(&mut self, key: i32) {
        if let Some(index) = self.entries.iter().position(|e| e.0 == key) {
            let (_, _, weight) = self.entries.remove(index);
            self.remaining += weight;
        }
    }

    /// Insert a stream of the given weight and return the streams evicted to
    /// make room.
    fn put(&mut self, value: StreamRef, key: i32, weight: i32) -> Result<Vec<StreamRef>, String> {
        if weight > self.capacity {
            return Err(format!(
                "stream weight {weight} exceeds the cache capacity {}",
                self.capacity
            ));
        }
        self.remove(key);
        self.remaining -= weight;
        let mut evicted = Vec::new();
        while self.remaining < 0 {
            if self.entries.is_empty() {
                return Err("cache ran empty while evicting".into());
            }
            let (_, value, weight) = self.entries.remove(0);
            self.remaining += weight;
            evicted.push(value);
        }
        self.entries.push((key, value, weight));
        Ok(evicted)
    }
}

/// A message for the server that the API queued; the owner sends it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outgoing {
    /// The song with this id ended (`SOUND_SONGEND`).
    SongEnd(i32),
    /// The song with this id has loaded or started (`SOUND_SONGPRELOADED`).
    SongPreloaded(i32),
}

/// What a sound request asks for. `volume` and `rate` are 0-255 (a rate of
/// 255 plays at the file's own speed); positional shapes need a `position`.
#[derive(Clone, Copy, Debug)]
pub struct SoundParams {
    pub kind: SoundType,
    pub id: i32,
    /// Plays; 0 and 1 play once, more repeat, negative loops forever.
    pub loops: i32,
    pub volume: i32,
    pub bus: i32,
    pub shape: SoundShape,
    /// The distance up to which a positioned sound is at full volume, and
    /// the distance beyond which it is silent.
    pub size: f32,
    pub range: f32,
    pub position: Option<[f32; 3]>,
    pub rate: i32,
}

impl SoundParams {
    /// An unpositioned request for sound `id` at full volume and normal
    /// rate, once, on `bus`.
    pub fn new(kind: SoundType, id: i32, bus: i32) -> Self {
        Self {
            kind,
            id,
            loops: 1,
            volume: 255,
            bus,
            shape: SoundShape::None,
            size: 0.0,
            range: 0.0,
            position: None,
            rate: 255,
        }
    }

    /// Set how many times the sound plays (see [`SoundParams::loops`]).
    pub fn with_loops(mut self, loops: i32) -> Self {
        self.loops = loops;
        self
    }

    /// Set the 0-255 volume.
    pub fn with_volume(mut self, volume: i32) -> Self {
        self.volume = volume;
        self
    }

    /// Set the playback rate.
    pub fn with_rate(mut self, rate: i32) -> Self {
        self.rate = rate;
        self
    }

    /// Place the sound in the world: `shape` attenuates it between `size`
    /// and `range` around `position`.
    pub fn positioned(
        mut self,
        shape: SoundShape,
        position: [f32; 3],
        size: f32,
        range: f32,
    ) -> Self {
        self.shape = shape;
        self.position = Some(position);
        self.size = size;
        self.range = range;
        self
    }
}

/// The client's audio owner.
pub struct AudioApi {
    /// The sound pool, capped at `max_sounds`.
    pool: Vec<Sound>,
    /// Pool indices of the sounds the API owns and is playing.
    sounds: Vec<usize>,
    /// Streams waiting for their group to load.
    pending_effects: HashMap<i32, StreamRef>,
    pending_songs: HashMap<i32, StreamRef>,
    /// Streams that finished loading this update.
    loaded: Vec<StreamRef>,
    /// The effect cache (7 MB of groups; evicting a stream releases the
    /// sounds using it) and the song cache (10 songs).
    effect_cache: Lru,
    song_cache: Lru,
    max_sounds: usize,
    /// The local player's position, the origin of positioned sounds.
    listener: Option<[f32; 3]>,
    /// Sound groups started and stopped together by scripts.
    groups: HashMap<i32, Vec<usize>>,
    /// The song that is playing (-1: none) and its sound.
    current_song: i32,
    current_song_sound: Option<usize>,
    /// A song being preloaded, and whether the server was told.
    preload: Option<usize>,
    preload_sent: bool,
    title_song: i32,
    /// The jingle that is playing, if any.
    jingle_song: i32,
    jingle_playing: bool,
    backend: SoundBackend,
    loader: Arc<SoundLoader>,
    /// Messages for the server, drained by the owner.
    pub outgoing: Vec<Outgoing>,
    /// The client is in the game state (song messages are only sent then).
    pub in_game: bool,
    /// The time of the current update.
    now: i64,
    /// Every sound request `create_sound` received, as asked (before the
    /// range test and clamping), for tests.
    #[cfg(any(test, feature = "test-hooks"))]
    requests: Vec<SoundParams>,
}

impl AudioApi {
    /// An API with a 50-sound pool and a 7 MB effect cache.
    pub fn new(loader: Arc<SoundLoader>, output: Output, spawn: bool, now: i64) -> Self {
        let mut api = Self {
            pool: Vec::new(),
            sounds: Vec::new(),
            pending_effects: HashMap::new(),
            pending_songs: HashMap::new(),
            loaded: Vec::new(),
            effect_cache: Lru::new(7_340_032),
            song_cache: Lru::new(10),
            max_sounds: 50,
            listener: None,
            groups: HashMap::new(),
            current_song: -1,
            current_song_sound: None,
            preload: None,
            preload_sent: false,
            title_song: 0,
            jingle_song: 0,
            jingle_playing: false,
            backend: SoundBackend::new(output, spawn),
            loader,
            outgoing: Vec::new(),
            in_game: false,
            now,
            #[cfg(any(test, feature = "test-hooks"))]
            requests: Vec::new(),
        };
        api.configure_routing_architecture();
        api
    }

    /// Build the bus tree: five main busses under the master (effects,
    /// music, login music, ambient, voice-over) and the sub-busses below
    /// them that sounds are routed to.
    fn configure_routing_architecture(&mut self) {
        let buss = &mut self.backend.buss;
        let master = buss_type::MASTER;
        buss.add(
            buss_type::SFX,
            master,
            0.2,
            Some(VolumeProvider::MainEffects),
        );
        buss.add(
            buss_type::MUSIC,
            master,
            1.0,
            Some(VolumeProvider::MainMusic),
        );
        buss.add(
            buss_type::MUSIC_LOGIN,
            master,
            1.0,
            Some(VolumeProvider::LoginMusic),
        );
        buss.add(
            buss_type::AMBIENT,
            master,
            0.8,
            Some(VolumeProvider::BackgroundEffects),
        );
        buss.add(
            buss_type::VOICEOVER,
            master,
            1.0,
            Some(VolumeProvider::Speech),
        );
        buss.add(sub_buss_type::MUSIC_SUB, buss_type::MUSIC, 1.0, None);
        buss.add(sub_buss_type::DIALOG_SUB, buss_type::VOICEOVER, 1.0, None);
        buss.add(sub_buss_type::SFX_SUB, buss_type::SFX, 1.0, None);
        buss.add(
            sub_buss_type::PLAYER_ANIMATION_SUB,
            buss_type::SFX,
            1.0,
            None,
        );
        buss.add(sub_buss_type::NPC_ANIMATION_SUB, buss_type::SFX, 1.0, None);
        buss.add(
            sub_buss_type::LOCATION_ANIMATION_SUB,
            buss_type::SFX,
            1.0,
            None,
        );
        buss.add(
            sub_buss_type::GENERAL_ANIMATION_SUB,
            buss_type::SFX,
            1.0,
            None,
        );
        buss.add(sub_buss_type::LOCATIONS_SUB, buss_type::AMBIENT, 1.0, None);
        buss.add(
            sub_buss_type::LOCATION_GENERIC_SUB,
            sub_buss_type::LOCATIONS_SUB,
            1.0,
            None,
        );
        buss.add(
            sub_buss_type::LOCATION_RANDOM_SUB,
            sub_buss_type::LOCATIONS_SUB,
            1.0,
            None,
        );
        buss.ease(sub_buss_type::MUSIC_SUB, 0.75, self.now);
    }

    /// The backend, for tests driving its tasks.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn backend(&self) -> &SoundBackend {
        &self.backend
    }

    /// One client cycle: ease the busses, load pending streams, advance
    /// every sound and run the song and jingle state machine. `camera`
    /// carries the camera fields of [`AdjustContext`]; `local_player` is the
    /// local player's position when there is one.
    pub fn update(
        &mut self,
        now: i64,
        prefs: &VolumePreferences,
        camera: &AdjustContext,
        local_player: Option<[f32; 3]>,
    ) {
        self.now = now;
        let ctx = AdjustContext {
            listener: self.listener,
            ..*camera
        };
        self.backend.update(now, prefs, &ctx);
        if let Some([x, _, z]) = local_player {
            self.listener = Some([x, 0.0, z]);
        }
        let pending: Vec<StreamRef> = self.pending_effects.values().cloned().collect();
        for stream in pending {
            let loaded = stream.borrow_mut().load();
            if let Some(weight) = loaded {
                self.loaded.push(stream.clone());
                let id = stream.borrow().id;
                match self.effect_cache.put(stream.clone(), id, weight) {
                    Ok(evicted) => {
                        for evicted in evicted {
                            self.stream_evicted(&evicted);
                        }
                    }
                    Err(error) => log::warn!("[client910] audio stream {id} cache: {error}"),
                }
            }
        }
        let pending: Vec<StreamRef> = self.pending_songs.values().cloned().collect();
        for stream in pending {
            if stream.borrow_mut().load().is_some() {
                self.loaded.push(stream.clone());
                let id = stream.borrow().id;
                if let Err(error) = self.song_cache.put(stream.clone(), id, 1) {
                    log::warn!("[client910] audio song {id} cache: {error}");
                }
            }
        }
        for stream in std::mem::take(&mut self.loaded) {
            let stream = stream.borrow();
            if stream.song {
                self.pending_songs.remove(&stream.id);
            } else {
                self.pending_effects.remove(&stream.id);
            }
        }
        if let Some(preload) = self.preload {
            if self.pool[preload].state == SoundState::Loaded && !self.preload_sent && self.in_game
            {
                if let Some(id) = self.pool[preload].stream_id() {
                    self.outgoing.push(Outgoing::SongPreloaded(id));
                }
                self.preload_sent = true;
            }
        }
        let ctx = AdjustContext {
            listener: self.listener,
            ..*camera
        };
        for key in 0..self.pool.len() {
            let effect_cache = &mut self.effect_cache;
            let mut touch = |id: i32| {
                effect_cache.get(id);
            };
            self.pool[key].tick_delay();
            self.pool[key].update(key, &self.backend, &ctx, now, &mut touch);
            let state = self.pool[key].state;
            if !self.pool[key].owned {
                continue;
            }
            if state == SoundState::Stopped || state == SoundState::Finished {
                let kind = self.pool[key].kind;
                if kind != Some(SoundType::Song) && kind != Some(SoundType::Jingle) {
                    for members in self.groups.values_mut() {
                        if let Some(index) = members.iter().position(|&m| m == key) {
                            members.remove(index);
                            break;
                        }
                    }
                    self.release(key);
                } else if self.preload == Some(key) {
                    self.preload_sent = false;
                    self.preload = None;
                    self.release(key);
                } else {
                    let id = self.pool[key].stream_id().unwrap_or(-1);
                    let music = self.mix_buss_volume(sub_buss_type::MUSIC_SUB) > 0.0;
                    if !self.jingle_playing && music {
                        if self.current_song == id {
                            self.send_song_end(id);
                            self.current_song = -1;
                        }
                        self.release(key);
                    } else if self.jingle_song == id {
                        self.jingle_song = -1;
                        self.jingle_playing = false;
                        self.release(key);
                        for other in 0..self.pool.len() {
                            let sound = &self.pool[other];
                            if sound.kind != Some(SoundType::Song) {
                                continue;
                            }
                            let song = sound.stream_id().unwrap_or(-1);
                            let resumable = self.current_song == song
                                && sound.state == SoundState::Stopped
                                || matches!(
                                    sound.state,
                                    SoundState::Created | SoundState::Starting | SoundState::Loaded
                                );
                            if resumable {
                                if sound.state == SoundState::Stopped {
                                    self.pool[other].resume(other);
                                } else {
                                    self.pool[other].start();
                                }
                                break;
                            }
                        }
                    } else if music {
                        if !self.jingle_playing || self.current_song != id {
                            self.release(key);
                        }
                        if !self.jingle_playing && self.current_song == id {
                            self.current_song = -1;
                            self.current_song_sound = None;
                        }
                    }
                }
            } else if state != SoundState::FadingOut
                && self.pool[key].bus == sub_buss_type::MUSIC_SUB
            {
                let audible = self.mix_buss_volume(sub_buss_type::MUSIC_SUB) > 1.0e-4;
                if !audible {
                    let now = self.now;
                    self.pool[key].fade_out(key, 150, now);
                }
            }
        }
    }

    /// Release a sound and drop it from the playing list.
    fn release(&mut self, key: usize) {
        let now = self.now;
        self.pool[key].release(key, now);
        if let Some(index) = self.sounds.iter().position(|&s| s == key) {
            self.sounds.remove(index);
        }
    }

    /// A stream left the effect cache: release the sounds playing from it.
    fn stream_evicted(&mut self, stream: &StreamRef) {
        let keys: Vec<usize> = self
            .sounds
            .iter()
            .copied()
            .filter(|&key| {
                self.pool[key]
                    .stream
                    .as_ref()
                    .is_some_and(|s| Rc::ptr_eq(s, stream))
            })
            .collect();
        for key in keys {
            let now = self.now;
            self.pool[key].release(key, now);
            self.sounds.retain(|&s| s != key);
        }
    }

    /// Add a sound to a group, held for `delay` until the group starts.
    pub fn add_sound_to_group(&mut self, key: usize, group: i32, delay: i32) {
        let members = self.groups.entry(group).or_default();
        if !members.contains(&key) {
            self.pool[key].set_delay(delay, true);
            members.push(key);
        }
    }

    /// Start every sound of a group.
    pub fn start_group(&mut self, group: i32) {
        let Some(members) = self.groups.get(&group) else {
            return;
        };
        for &key in members {
            if !self.pool[key].release_hold() {
                self.pool[key].start();
            }
        }
    }

    /// Fade out every sound of a group.
    pub fn stop_group(&mut self, group: i32) {
        let Some(members) = self.groups.get(&group) else {
            return;
        };
        for &key in members {
            self.pool[key].fade_out(key, 50, self.now);
        }
    }

    /// Load the streams of a group without starting it.
    pub fn preload_sound_group(&mut self, group: i32) {
        let Some(members) = self.groups.get(&group) else {
            return;
        };
        for &key in members {
            self.pool[key].preload();
        }
    }

    /// Add a script-defined bus under `parent` with a level in 1/65536ths.
    pub fn add_buss(&mut self, id: i32, parent: i32, level: i32) {
        let master = buss_type::MASTER;
        let buss = &mut self.backend.buss;
        if buss.get(id).is_none() && (parent == master || buss.get(parent).is_some()) {
            let priority = level as f32 * 1.5258789E-5;
            buss.add(
                id,
                if parent == master { -1 } else { parent },
                priority,
                None,
            );
        }
    }

    /// Ease a bus to `level` (1/65536ths).
    pub fn set_mix_buss_level(&mut self, id: i32, level: i32) {
        self.backend
            .buss
            .ease(id, level as f32 * 1.5258789E-5, self.now);
    }

    /// The product of a bus's volume and its ancestors'.
    pub fn mix_buss_volume(&self, id: i32) -> f32 {
        let mut volume = 1.0;
        let mut current = self.backend.buss.get(id);
        while let Some(buss) = current {
            volume *= buss.self_volume();
            current = buss.parent().and_then(|p| self.backend.buss.get(p));
        }
        volume
    }

    /// Fade out the sounds on `bus` and busses related to it.
    pub fn stop_vorbis_speech(&mut self, bus: i32) {
        for key in self.sounds.clone() {
            if self
                .backend
                .buss
                .common_parent_exists(self.pool[key].bus, bus)
            {
                self.pool[key].fade_out(key, 50, self.now);
            }
        }
    }

    /// Load an effect's group ahead of use.
    pub fn preload_sounds(&mut self, id: i32) {
        if id >= 0 {
            self.stream(id, false);
        }
    }

    /// The stream for a song or effect id: cached, pending, or new.
    fn stream(&mut self, id: i32, song: bool) -> StreamRef {
        let cached = if song {
            self.song_cache.get(id)
        } else {
            self.effect_cache.get(id)
        };
        let pending = if song {
            &mut self.pending_songs
        } else {
            &mut self.pending_effects
        };
        if let Some(stream) = cached.or_else(|| pending.get(&id).cloned()) {
            return stream;
        }
        let stream = Rc::new(RefCell::new(Stream::new(song, id, self.loader.clone())));
        pending.insert(id, stream.clone());
        stream
    }

    /// A sound for `stream`: a released one from the pool, or a new one
    /// while the pool has room.
    fn claim_sound(&mut self, stream: StreamRef) -> Option<usize> {
        if let Some(key) = self
            .pool
            .iter()
            .position(|s| s.state == SoundState::Released)
        {
            self.pool[key].attach(stream);
            return Some(key);
        }
        if self.pool.len() >= self.max_sounds {
            return None;
        }
        self.pool.push(Sound::new(stream));
        Some(self.pool.len() - 1)
    }

    /// Hand a sound to the API: it plays and is reaped when it stops.
    fn play(&mut self, key: usize) {
        self.pool[key].owned = true;
        self.sounds.push(key);
    }

    /// Create a sound from `params` without starting it. `song` selects the
    /// song archive and cache. A positioned sound outside its range, or
    /// without a listener, is not created.
    pub fn create_sound(&mut self, params: &SoundParams, song: bool) -> Option<usize> {
        #[cfg(any(test, feature = "test-hooks"))]
        self.requests.push(*params);
        let SoundParams {
            kind,
            id,
            loops,
            volume,
            bus,
            shape,
            size,
            range,
            position,
            rate,
        } = *params;
        let volume = volume.clamp(0, 255);
        let mut rate = rate.max(0);
        if shape != SoundShape::None {
            let (Some(listener), Some(position)) = (self.listener, position) else {
                return None;
            };
            if length(sub(position, listener)) >= range {
                return None;
            }
        }
        if rate <= 0 {
            rate = 255;
        }
        let volume = volume as f32 / 255.0;
        let rate = rate as f32 / 255.0;
        let stream = self.stream(id, song);
        let key = self.claim_sound(stream)?;
        let now = self.now;
        let sound = &mut self.pool[key];
        sound.owned = true;
        sound.set_bus(bus);
        if shape != SoundShape::None {
            let position = position.unwrap_or_default();
            sound.position = Some(position);
            sound.size = size;
            sound.range = range;
            sound.set_adjuster(Some(Adjuster {
                shape,
                position,
                size,
                range,
            }));
        }
        sound.set_volume(key, volume, 0, now);
        sound.set_looping(
            !(0..=1).contains(&loops),
            if loops > 0 { loops - 1 } else { loops },
        );
        sound.set_rate(rate);
        sound.kind = Some(kind);
        Some(key)
    }

    /// Create a sound owned by the caller (a positioned sound or a cutscene
    /// action) rather than by the API: `update` keeps it after it stops
    /// until the owner hands it over with [`Self::sound_play`].
    pub fn create_owned_sound(&mut self, params: &SoundParams) -> Option<usize> {
        let key = self.create_sound(params, false)?;
        self.pool[key].owned = false;
        Some(key)
    }

    /// Hand a caller-owned sound to the API.
    pub fn sound_play(&mut self, key: usize) {
        if key < self.pool.len() {
            self.play(key);
        }
    }

    /// The state of a sound; a sound that no longer exists is released.
    pub fn sound_status(&self, key: usize) -> SoundState {
        self.pool
            .get(key)
            .map_or(SoundState::Released, |sound| sound.state)
    }

    /// Fade a sound to `volume` over `ms` milliseconds.
    pub fn sound_set_volume(&mut self, key: usize, volume: f32, ms: i32) {
        let now = self.now;
        if let Some(sound) = self.pool.get_mut(key) {
            sound.set_volume(key, volume, ms, now);
        }
    }

    /// Move a positioned sound.
    pub fn sound_set_position(&mut self, key: usize, position: [f32; 3]) {
        if let Some(sound) = self.pool.get_mut(key) {
            sound.set_position(key, position);
        }
    }

    /// The listener the positional range test and the adjusters use.
    pub fn listener(&self) -> Option<[f32; 3]> {
        self.listener
    }

    /// Create and play a sound, starting after `delay` updates (at once when
    /// 0).
    pub fn play_sound(&mut self, params: &SoundParams, delay: i32) {
        let Some(key) = self.create_sound(params, false) else {
            return;
        };
        if delay == 0 {
            self.pool[key].start();
        } else {
            self.pool[key].set_delay(delay, false);
        }
        self.play(key);
    }

    /// Load a created sound's stream without starting it.
    pub fn sound_preload(&mut self, key: usize) {
        if let Some(sound) = self.pool.get_mut(key) {
            sound.preload();
        }
    }

    /// Start a created sound.
    pub fn sound_start(&mut self, key: usize) {
        if let Some(sound) = self.pool.get_mut(key) {
            sound.start();
        }
    }

    /// Fade a caller-owned sound out and hand it to the API to finish.
    pub fn sound_fade_out_play(&mut self, key: usize, ms: i32) {
        let now = self.now;
        if key < self.pool.len() {
            self.pool[key].fade_out(key, ms, now);
            self.play(key);
        }
    }

    /// Load a song ahead of use; the server is told when it is ready.
    pub fn preload_song(&mut self, id: i32, volume: i32) {
        if self.current_song == id {
            return;
        }
        if let Some(preload) = self.preload.take() {
            self.pool[preload].fade_out(preload, 0, self.now);
            self.play(preload);
        }
        let params = SoundParams {
            loops: 0,
            volume,
            ..SoundParams::new(SoundType::Song, id, sub_buss_type::MUSIC_SUB)
        };
        if let Some(key) = self.create_sound(&params, true) {
            self.pool[key].preload();
            self.preload = Some(key);
        }
        self.preload_sent = false;
    }

    /// Set the song of the title screen.
    pub fn set_title_song(&mut self, id: i32) {
        self.title_song = id;
    }

    /// The song of the title screen.
    pub fn title_song(&self) -> i32 {
        self.title_song
    }

    /// The song that is playing (-1: none).
    pub fn current_song(&self) -> i32 {
        self.current_song
    }

    /// Tell the server a song ended (in the game state only).
    fn send_song_end(&mut self, id: i32) {
        if self.in_game {
            self.outgoing.push(Outgoing::SongEnd(id));
        }
    }

    /// Play a song, fading out the previous one. `located` carries a
    /// located song's `(x, z, size, range)` in fine units.
    pub fn play_song(&mut self, id: i32, volume: i32, located: Option<[i32; 4]>) {
        if self.current_song == id {
            return;
        }
        let now = self.now;
        if self.jingle_playing && self.current_song != id {
            if let Some(song) = self.current_song_sound {
                self.pool[song].resume(song);
                self.release(song);
            }
        }
        if let Some(playing) = self.playing_song() {
            if self.pool[playing].stream_id() == Some(id) {
                self.current_song_sound = Some(playing);
                self.current_song = id;
                return;
            }
        }
        let mut crossfade = false;
        if self.current_song >= 0 {
            for &key in &self.sounds {
                if self.pool[key].kind == Some(SoundType::Song) {
                    self.pool[key].fade_out(key, 2000, now);
                    crossfade = true;
                }
            }
        }
        self.current_song_sound = None;
        self.current_song = -1;
        let mut sound = None;
        if let Some(preload) = self.preload {
            let matches = self.pool[preload].stream_id() == Some(id)
                && self.pool[preload].state == SoundState::Loaded;
            if matches {
                sound = Some(preload);
                self.preload = None;
                self.preload_sent = false;
            }
        }
        let bus = if self.title_song == id {
            buss_type::MUSIC_LOGIN
        } else {
            sub_buss_type::MUSIC_SUB
        };
        let start_volume = if crossfade { 0 } else { volume };
        if sound.is_none() {
            let base = SoundParams {
                loops: 0,
                volume: start_volume,
                ..SoundParams::new(SoundType::Song, id, bus)
            };
            let params = match located {
                Some([x, z, size, range]) => SoundParams {
                    shape: SoundShape::Volume,
                    size: size as f32,
                    range: range as f32,
                    position: Some([x as f32, 0.0, z as f32]),
                    ..base
                },
                None => base,
            };
            sound = self.create_sound(&params, true);
        }
        let Some(key) = sound else {
            return;
        };
        if crossfade {
            self.pool[key].set_volume(key, volume as f32 / 255.0, 2000, now);
        }
        self.pool[key].start();
        self.play(key);
        self.current_song_sound = Some(key);
        self.current_song = id;
        if self.jingle_playing {
            self.pool[key].pause(key);
        }
        if self.in_game {
            self.outgoing
                .push(Outgoing::SongPreloaded(self.current_song));
        }
    }

    /// The song sound that is currently playing.
    fn playing_song(&self) -> Option<usize> {
        self.sounds.iter().copied().find(|&key| {
            self.pool[key].kind == Some(SoundType::Song)
                && self.pool[key].state == SoundState::Playing
        })
    }

    /// Fade the song out.
    pub fn stop_song(&mut self) {
        let now = self.now;
        for key in self.sounds.clone() {
            if self.pool[key].kind == Some(SoundType::Song) {
                self.pool[key].fade_out(key, 500, now);
                if self.pool[key].stream_id() == Some(self.current_song) {
                    let id = self.current_song;
                    self.send_song_end(id);
                    break;
                }
            }
        }
        self.current_song_sound = None;
        self.current_song = -1;
    }

    /// Play a jingle, pausing the song underneath it.
    pub fn play_jingle(&mut self, id: i32, volume: i32) {
        if self.jingle_playing && self.jingle_song == id {
            return;
        }
        if self.jingle_playing && self.jingle_song != id {
            if let Some(key) = self
                .pool
                .iter()
                .position(|s| s.kind == Some(SoundType::Jingle))
            {
                self.release(key);
                self.jingle_playing = false;
            }
        }
        if volume == 0 || id == -1 {
            return;
        }
        if !self.jingle_playing {
            if let Some(song) = self.current_song_sound {
                self.pool[song].pause(song);
            }
        }
        let params = SoundParams {
            loops: 0,
            volume,
            ..SoundParams::new(SoundType::Jingle, id, sub_buss_type::MUSIC_SUB)
        };
        if let Some(key) = self.create_sound(&params, true) {
            self.pool[key].start();
            self.play(key);
            self.jingle_playing = true;
            self.jingle_song = id;
        }
    }

    /// Diagnostics for tests.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn sound_state(&self, key: usize) -> SoundState {
        self.pool[key].state
    }

    #[cfg(any(test, feature = "test-hooks"))]
    pub fn active_sounds(&self) -> &[usize] {
        &self.sounds
    }

    /// The sound requests received since the last call.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn take_requests(&mut self) -> Vec<SoundParams> {
        std::mem::take(&mut self.requests)
    }
}
