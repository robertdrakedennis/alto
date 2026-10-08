//! The sound backend: the 50-voice pool, the loudness ranking that decides
//! which voices keep playing, and the two worker threads (decode ahead, and
//! mix and write to the output).
//!
//! Each player writes to an [`audio_sink::Sink`](crate::audio_sink::Sink)
//! chosen by [`Output`]. The busses are in `audio_bus`, the mixer and its
//! samples in `audio_mixer`.

use crate::audio_sink::{spawn_device, Device, DEVICE_FAILED, DEVICE_OPENING};
use crate::audio_voice::Voice;
use crate::audio_vorbis::{Endianness, SampleFormat, SetupCache, VorbisDecoder};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub use crate::audio_bus::{
    buss_type, sub_buss_type, AudioBuss, BussManager, VolumePreferences, VolumeProvider,
};
pub use crate::audio_mixer::{
    play_sample, remove_sample, sample_free, write_sample, LineMixer, Players, SampleHandle,
};
#[cfg(any(test, feature = "test-hooks"))]
pub use crate::audio_sink::RecordingSink;
pub use crate::audio_sink::{lock, ClockSink, DeviceSink, Sink};

pub type VoiceRef = Arc<Mutex<Voice>>;

/// State shared with the two worker threads.
struct Shared {
    players: Players,
    voices: Vec<VoiceRef>,
    stop: AtomicBool,
}

/// One output-thread turn: refresh the line space, hand each voice's PCM to
/// its mixer sample, mix and write.
fn task1_step(shared: &Shared) {
    for player in shared.players.iter() {
        lock(player).refresh_available();
    }
    for voice in &shared.voices {
        lock(voice).output();
    }
    for player in shared.players.iter() {
        lock(player).flush();
    }
}

/// One decode-thread turn: decode ahead for every voice. A voice whose
/// decoder fails is finished and reported instead of ending the thread.
fn task2_step(shared: &Shared) {
    for voice in &shared.voices {
        let mut voice = lock(voice);
        if let Err(error) = voice.decode_tick() {
            log::warn!("[client910] audio voice decode failed: {error:#}");
            voice.fail();
        }
    }
}

/// How the backend's player lines reach the outside world.
pub enum Output {
    /// The default `cpal` device, degrading to clock timing (logged once)
    /// while it opens and when none is usable.
    Device,
    /// [`ClockSink`]s only (headless).
    Clock,
    /// Caller-supplied lines, in player order (44.1 kHz, 22.05 kHz).
    #[cfg(any(test, feature = "test-hooks"))]
    Lines(Vec<Box<dyn Sink>>),
}

/// The voice pool and its threads. The client thread owns the bus manager
/// and the voice ranking; the worker threads share the voices and players.
pub struct SoundBackend {
    shared: Arc<Shared>,
    pub buss: BussManager,
    /// The voices, ranked by loudness (loudest first).
    order: Vec<VoiceRef>,
    threads: Vec<JoinHandle<()>>,
    _device: Option<Device>,
}

/// The two players: sample rate and line buffer size in bytes. A sound is
/// mixed by the player whose rate is nearest its own.
const PLAYERS: [(f32, i32); 2] = [(44100.0, 32768), (22050.0, 16384)];

impl SoundBackend {
    /// A backend with a pool of 50 voices. `spawn` starts the two worker
    /// threads; tests drive [`Self::step_output`]/[`Self::step_decode`].
    pub fn new(output: Output, spawn: bool) -> Self {
        let mut device = None;
        let lines: Vec<Box<dyn Sink>> = match output {
            #[cfg(any(test, feature = "test-hooks"))]
            Output::Lines(lines) => lines,
            Output::Clock => PLAYERS
                .iter()
                .map(|&(rate, size)| Box::new(ClockSink::new(rate, size)) as Box<dyn Sink>)
                .collect(),
            Output::Device => {
                let status = Arc::new(std::sync::atomic::AtomicU8::new(DEVICE_OPENING));
                let rings: Vec<_> = PLAYERS
                    .iter()
                    .map(|&(rate, _)| (Arc::new(Mutex::new(VecDeque::new())), rate))
                    .collect();
                match spawn_device(rings.clone(), status.clone()) {
                    Ok(spawned) => device = Some(spawned),
                    Err(error) => {
                        status.store(DEVICE_FAILED, Ordering::Release);
                        log::warn!("[client910] audio device thread unavailable: {error}");
                    }
                }
                rings
                    .into_iter()
                    .zip(PLAYERS)
                    .map(|((ring, rate), (_, size))| {
                        Box::new(DeviceSink {
                            ring,
                            capacity: (size / 2) as usize,
                            status: status.clone(),
                            clock: ClockSink::new(rate, size),
                        }) as Box<dyn Sink>
                    })
                    .collect()
            }
        };
        let players: Players = Arc::new(
            PLAYERS
                .iter()
                .zip(lines)
                .map(|(&(rate, size), line)| Mutex::new(LineMixer::new(rate, size, line)))
                .collect(),
        );
        // One codebook cache for the whole pool: sounds from one encoder
        // configuration parse their codebooks once.
        let setups = SetupCache::default();
        let voices: Vec<VoiceRef> = (0..50)
            .map(|_| {
                let mut decoder = VorbisDecoder::with_setup_cache(2.0, setups.clone());
                decoder.configure(SampleFormat::Signed16, Endianness::Little, 2);
                Arc::new(Mutex::new(Voice::new(8192, 3, decoder, players.clone())))
            })
            .collect();
        let shared = Arc::new(Shared {
            players,
            voices: voices.clone(),
            stop: AtomicBool::new(false),
        });
        let mut threads = Vec::new();
        if spawn {
            for (name, period, task) in [
                ("client910-audio-decode", 10_u64, task2_step as fn(&Shared)),
                ("client910-audio-output", 6_u64, task1_step as fn(&Shared)),
            ] {
                let shared = shared.clone();
                let spawned = std::thread::Builder::new()
                    .name(name.into())
                    .spawn(move || {
                        while !shared.stop.load(Ordering::Acquire) {
                            task(&shared);
                            std::thread::sleep(Duration::from_millis(period));
                        }
                    });
                match spawned {
                    Ok(handle) => threads.push(handle),
                    Err(error) => log::warn!("[client910] audio task {name} unavailable: {error}"),
                }
            }
        }
        Self {
            shared,
            buss: BussManager::default(),
            order: voices,
            threads,
            _device: device,
        }
    }

    /// One output-thread turn (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn step_output(&self) {
        task1_step(&self.shared);
    }

    /// One decode-thread turn (tests).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn step_decode(&self) {
        task2_step(&self.shared);
    }

    /// The per-cycle update: bus eases, each voice's client update, the
    /// loudness ranking, and the cull that stops silent voices and keeps six
    /// voices of headroom free.
    pub fn update(
        &mut self,
        now: i64,
        prefs: &VolumePreferences,
        ctx: &crate::audio_adjust::AdjustContext,
    ) {
        self.buss.update(now, prefs);
        let mut changed = false;
        for voice in &self.order {
            let mut voice = lock(voice);
            voice.client_update(now, &self.buss, ctx);
            changed |= voice.volume_changed();
        }
        if changed {
            let mut keyed: Vec<(f32, VoiceRef)> = self
                .order
                .iter()
                .map(|v| (lock(v).priority_volume(), v.clone()))
                .collect();
            // Loudest first; equal voices keep their order.
            keyed.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            self.order = keyed.into_iter().map(|(_, v)| v).collect();
        }
        let headroom = 6_i32;
        let len = self.order.len() as i32;
        let mut index = len - 1;
        let mut found = false;
        while !found {
            let mut voice = lock(&self.order[index as usize]);
            if voice.priority_volume() >= 0.0 {
                found = true;
            } else if voice.state().is_active_for_cull() {
                voice.stop();
            }
            drop(voice);
            index -= 1;
            if index < 0 {
                found = true;
            }
        }
        if index < len - headroom {
            return;
        }
        while index >= len - headroom {
            let mut voice = lock(&self.order[index as usize]);
            if voice.state() == crate::audio_voice::VoiceState::Playing {
                voice.stop();
            }
            index -= 1;
        }
    }

    /// Claim a free voice.
    pub fn allocate(&self) -> Option<VoiceRef> {
        for voice in &self.order {
            let mut guard = lock(voice);
            if guard.state() == crate::audio_voice::VoiceState::Free {
                guard.allocate();
                drop(guard);
                return Some(voice.clone());
            }
        }
        None
    }
}

impl Drop for SoundBackend {
    /// Stop the worker threads.
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Release);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_sink::DEVICE_RUNNING;
    use std::time::Instant;

    /// Plays silence through the `cpal` sink for a moment.
    #[test]
    #[ignore = "diagnostic: opens the default audio output device (hardware smoke)"]
    fn device_sink_opens() {
        let backend = SoundBackend::new(Output::Device, true);
        let status = backend
            ._device
            .as_ref()
            .expect("device thread")
            .status
            .clone();
        let start = Instant::now();
        while status.load(Ordering::Acquire) == DEVICE_OPENING
            && start.elapsed() < Duration::from_secs(10)
        {
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(status.load(Ordering::Acquire), DEVICE_RUNNING);
        std::thread::sleep(Duration::from_millis(300));
        drop(backend);
    }
}
