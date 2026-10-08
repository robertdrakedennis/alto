//! The client's audio owner: routes every retained sound request (CS2
//! `sound_*` hosts, server audio packets, zone `SOUND_AREA` and sequence
//! sounds) into the [`AudioApi`], runs its update once per client cycle
//! and queues the `SOUND_SONGEND`/`SOUND_SONGPRELOADED` messages it emits.
//!
//! The decoder, voices, busses, mixer and output sink live in the
//! rs910-audio crate.

use crate::audio_api::{AdjustContext, AudioApi, Outgoing, SoundParams, SoundShape};
use crate::audio_backend::{sub_buss_type, Output, VolumePreferences};
use crate::audio_stream::{SoundLoader, SoundType};
use crate::cache::Pack;
use crate::positioned_sound::{FrameInput, PositionedSounds};
use rs910_game::sequence_sound::SequenceSound;
use std::collections::BTreeSet;
use std::sync::Arc;

/// One audio request retained until the client audio backend consumes
/// it. The command name is preserved because song/effect/group routing has
/// different lifecycle semantics even while the decoder/device owner is
/// still being completed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SoundRequest {
    pub command: String,
    pub args: Vec<i32>,
}

/// The local player as the audio owner samples it: its level and world
/// position (scene-local fine units).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub level: i32,
    pub x: f32,
    pub z: f32,
}

/// The client-side audio owner.
pub struct AudioRuntime {
    api: AudioApi,
    loader: Arc<SoundLoader>,
    prefs: VolumePreferences,
    listener: Option<Listener>,
    camera: AdjustContext,
    unknown: BTreeSet<String>,
    /// The sound held by each cutscene sound action, keyed by action index.
    cutscene_sounds: std::collections::BTreeMap<i32, usize>,
    /// The loc/NPC/player background sounds.
    pub positioned: PositionedSounds,
}

impl AudioRuntime {
    /// Build the owner over the cache and load the title song from the
    /// defaults archive.
    pub fn new(pack: Pack) -> Self {
        let test = cfg!(any(test, feature = "test-hooks"));
        let loader = SoundLoader::new(Some(PackGroups(pack.clone())), test);
        let output = if test { Output::Clock } else { Output::Device };
        let mut api = AudioApi::new(
            loader.clone(),
            output,
            !test,
            crate::logic_clock::monotonic_millis(),
        );
        match title_song(&pack) {
            Ok(Some(song)) => api.set_title_song(song),
            Ok(None) => {}
            Err(error) => log::warn!("[client910] audio defaults unavailable: {error:#}"),
        }
        Self {
            api,
            loader,
            prefs: VolumePreferences::default(),
            listener: None,
            camera: AdjustContext::default(),
            unknown: BTreeSet::new(),
            cutscene_sounds: std::collections::BTreeMap::new(),
            positioned: PositionedSounds::default(),
        }
    }

    /// One drawn frame of the background sounds, gated on the
    /// background-sound volume preference.
    pub fn positioned_frame(&mut self, input: &FrameInput<'_>) {
        self.positioned
            .frame(&mut self.api, input, self.prefs.background_sound);
    }

    /// Fade out and drop the loc sounds, and with `all` the NPC and player
    /// sounds too. Logout passes true.
    pub fn reset_positioned(&mut self, all: bool) {
        self.positioned.reset(&mut self.api, all);
    }

    /// `CLIENT910_POSAUDIO_TRACE`: every positioned sound with a live
    /// sound, its centre, listener distance and the stereo distance gain
    /// times `volume / 255`.
    pub fn trace_positioned(&self, cycle: i32, level: i32, local: [f32; 2]) {
        let listener = self.api.listener();
        let p = &self.positioned;
        log::info!(
            "[client910] posaudio cycle {cycle}: level {level} player ({:.0},{:.0}) listener {listener:?} entries loc {} npc {} player {} bg-volume {}",
            local[0],
            local[1],
            p.locs.len(),
            p.npcs.len(),
            p.players.len(),
            self.prefs.background_sound
        );
        let Some(listener) = listener else {
            return;
        };
        for entry in p.active() {
            let distance = |at: [f32; 3]| {
                ((at[0] - listener[0]).powi(2) + (at[2] - listener[2]).powi(2)).sqrt()
            };
            if let Some(key) = entry.loop_sound {
                let d = distance(entry.position);
                let gain =
                    crate::audio_api::attenuation(d, entry.dropoffrange as f32, entry.range as f32);
                log::info!(
                    "[client910] posaudio   {} loop sound {} at ({:.0},{:.0}) L{} dist {d:.0} dropoff {} range {} volume {} gain {:.3} -> {:.3} state {:?}",
                    PositionedSounds::owner_label(entry),
                    entry.sound,
                    entry.position[0],
                    entry.position[2],
                    entry.level,
                    entry.dropoffrange,
                    entry.range,
                    entry.volume,
                    gain,
                    gain * entry.volume as f32 / 255.0,
                    self.api.sound_status(key)
                );
            }
            if let Some(key) = entry.random_sound {
                let d = distance(entry.random_position);
                let range = (entry.range + entry.dropoffrange) as f32;
                let gain = crate::audio_api::attenuation(d, entry.dropoffrange as f32, range);
                log::info!(
                    "[client910] posaudio   {} random of {:?} dist {d:.0} gain {:.3} delay {} state {:?}",
                    PositionedSounds::owner_label(entry),
                    entry.random,
                    gain,
                    entry.delay,
                    self.api.sound_status(key)
                );
            }
        }
    }

    /// The volumes the providers currently read.
    #[must_use]
    pub fn volumes(&self) -> VolumePreferences {
        self.prefs
    }

    /// The volume preferences the audio volume providers read.
    pub fn update_preferences(&mut self, options: &crate::client_options::ClientOptions) {
        let get = |field: &str| options.get(field).unwrap_or(0).clamp(0, 255);
        self.prefs = VolumePreferences {
            sound: get("soundVolume"),
            background_sound: get("backgroundSoundVolume"),
            speech: get("speechVolume"),
            music: get("unknownVolume1"),
            login_music: get("unknownVolume2"),
        };
    }

    /// The local player the per-cycle update positions the listener at.
    pub fn update_listener(&mut self, listener: Option<Listener>) {
        self.listener = listener;
    }

    /// The camera state and free-camera yaw the stereo adjusters read.
    pub fn set_camera(&mut self, camera_state: i32, cam2_yaw: f32) {
        self.camera.camera_state = camera_state;
        self.camera.cam2_yaw = cam2_yaw;
    }

    /// The classic camera yaw (14-bit units) used outside the free-camera state.
    pub fn set_camera_yaw(&mut self, yaw: i32) {
        self.camera.camera_yaw = yaw;
    }

    /// Update the audio API for one client cycle. `in_game` is true while the
    /// game connection is up and the client is in the game state; queued
    /// client messages are appended to `outgoing`. Outside the game state the
    /// title song keeps playing at the login-music volume.
    pub fn tick(&mut self, now: i64, in_game: bool, outgoing: &mut Vec<u8>) {
        self.api.in_game = in_game;
        if !in_game {
            let title = self.api.title_song();
            self.api.play_song(title, self.prefs.login_music, None);
        }
        let local = self.listener.map(|l| [l.x, 0.0, l.z]);
        self.api.update(now, &self.prefs, &self.camera, local);
        for message in self.api.outgoing.drain(..) {
            let (opcode, value) = match message {
                Outgoing::SongEnd(id) => (crate::proto::client::SOUND_SONGEND, id),
                Outgoing::SongPreloaded(id) => (crate::proto::client::SOUND_SONGPRELOADED, id),
            };
            outgoing.push(opcode);
            outgoing.extend(value.to_be_bytes());
        }
    }

    /// Route one retained request to its audio API call.
    pub fn submit(&mut self, sound: &SoundRequest) {
        let a = |i: usize| sound.args.get(i).copied().unwrap_or(0);
        let api = &mut self.api;
        use sub_buss_type::{DIALOG_SUB, SFX_SUB};
        // Script-style requests share one argument layout:
        // id, loops, delay, volume, rate.
        let scripted = |kind, bus| {
            SoundParams::new(kind, a(0), bus)
                .with_loops(a(1))
                .with_volume(a(3))
                .with_rate(a(4))
        };
        match sound.command.as_str() {
            "midi_jingle" | "cutscene_jingle" => api.play_jingle(a(0), a(1)),
            // A cutscene song plays at full volume.
            "cutscene_song" => api.play_song(a(0), 255, None),
            "cutscene_song_preload" => api.preload_song(a(0), a(1)),
            // Arguments: [tag, id, repeats, volume, rate], on the effects bus.
            "cutscene_sound_create" => {
                // The action owns the sound until cleanup hands it over with
                // `play`.
                let params = SoundParams::new(SoundType::Script, a(1), SFX_SUB)
                    .with_loops(a(2))
                    .with_volume(a(3))
                    .with_rate(a(4));
                if let Some(key) = api.create_owned_sound(&params) {
                    api.sound_preload(key);
                    self.cutscene_sounds.insert(a(0), key);
                }
            }
            "cutscene_sound_start" => {
                if let Some(&key) = self.cutscene_sounds.get(&a(0)) {
                    api.sound_start(key);
                }
            }
            "cutscene_sound_cleanup" => {
                if let Some(key) = self.cutscene_sounds.remove(&a(0)) {
                    api.sound_fade_out_play(key, 50);
                }
            }
            "midi_song" | "sound_song_volume" => api.play_song(a(0), a(1), None),
            "sound_song" | "sound_jingle" => api.play_song(a(0), 255, None),
            "sound_jingle_volume" => api.play_song(a(0), a(2), None),
            "midi_song_stop" | "sound_song_stop" => api.stop_song(),
            // Coordinates and extents are in fine units.
            "midi_song_location" => api.play_song(
                a(0),
                a(1),
                Some([a(3) << 9, a(4) << 9, a(5) << 9, a(6) << 9]),
            ),
            "synth_sound" | "vorbis_sound" => {
                api.play_sound(&scripted(SoundType::Effect, SFX_SUB), a(2))
            }
            // The speech rate is fixed at 256.
            "vorbis_speech_sound" => api.play_sound(
                &scripted(SoundType::Effect, DIALOG_SUB).with_rate(256),
                a(2),
            ),
            "vorbis_speech_stop" => api.stop_vorbis_speech(DIALOG_SUB),
            "song_preload" => api.preload_song(a(0), 255),
            "vorbis_preload_sounds" => api.preload_sounds(a(0)),
            "vorbis_preload_sound_group" => api.preload_sound_group(a(0)),
            "vorbis_sound_group_start" | "sound_group_start" => api.start_group(a(0)),
            "vorbis_sound_group_stop" | "sound_group_stop" => api.stop_group(a(0)),
            // Arguments: id, loops, delay, volume, rate, group.
            "vorbis_sound_group" | "sound_vorbis_volume_rate_group" => {
                if let Some(key) = api.create_sound(&scripted(SoundType::Effect, SFX_SUB), false) {
                    api.add_sound_to_group(key, a(5), a(2));
                }
            }
            "sound_mixbuss_add" => api.add_buss(a(0), a(1), a(2)),
            "sound_mixbuss_setlevel" => api.set_mix_buss_level(a(0), a(1)),
            // The plain synth host plays at volume 255 and rate 256.
            "sound_synth" => api.play_sound(
                &SoundParams::new(SoundType::Script, a(0), SFX_SUB)
                    .with_loops(a(1))
                    .with_rate(256),
                a(2),
            ),
            // The volume variants play at rate 256 on the effects bus.
            "sound_synth_volume" | "sound_vorbis_volume" | "sound_speech_volume" => {
                api.play_sound(&scripted(SoundType::Script, SFX_SUB).with_rate(256), a(2))
            }
            "sound_synth_rate" | "sound_vorbis_rate" => {
                api.play_sound(&scripted(SoundType::Script, SFX_SUB), a(2))
            }
            // The distance-focus filter host only pops its five arguments.
            "sound_distancefocusfilter_setparams" => {}
            // Zone sound area. Arguments: [sound, loops, delay, volume, rate,
            // level, x, z, range, dialog]; the dialog flag selects the dialog bus.
            "sound_area" => {
                let bus = if a(9) != 0 { DIALOG_SUB } else { SFX_SUB };
                let position = [(a(6) << 9) as f32, 0.0, (a(7) << 9) as f32];
                api.play_sound(
                    &scripted(SoundType::Effect, bus).positioned(
                        SoundShape::Stereo,
                        position,
                        0.0,
                        (a(8) << 9) as f32,
                    ),
                    a(2),
                )
            }
            other => {
                if self.unknown.insert(other.to_string()) {
                    log::info!("[client910] audio request {other} has no route");
                }
            }
        }
    }

    /// Play a sequence sound the game raised for an entity (after its
    /// level and visibility gates). `active_target` is the server-set
    /// target (`SET_TARGET`).
    ///
    /// A sound with a range plays positioned at the entity's tile, on the
    /// effects bus of animations, unless its preference is muted: the local
    /// player's sounds follow the effects volume and everything else the
    /// background volume. One without a range plays only for the local
    /// player, unpositioned, on the player-animation bus.
    pub fn play_sequence_sound(&mut self, sound: &SequenceSound, active_target: i32) {
        use sub_buss_type::{NPC_ANIMATION_SUB, PLAYER_ANIMATION_SUB};
        if sound.range != 0 {
            let muted = if sound.local {
                self.prefs.sound == 0
            } else {
                self.prefs.background_sound == 0
            };
            if muted {
                return;
            }
            let volume = sequence_sound_volume(sound, active_target);
            let shape = if sound.local {
                SoundShape::None
            } else {
                SoundShape::Stereo
            };
            let params = SoundParams::new(SoundType::SequenceArea, sound.id, NPC_ANIMATION_SUB)
                .with_loops(sound.loops)
                .with_volume(volume)
                .with_rate(sound.rate)
                .positioned(
                    shape,
                    [sound.x as f32, 0.0, sound.z as f32],
                    0.0,
                    (sound.range << 9) as f32,
                );
            self.api.play_sound(&params, 0);
        } else if sound.local {
            let params = SoundParams::new(SoundType::SequenceLocal, sound.id, PLAYER_ANIMATION_SUB)
                .with_loops(sound.loops)
                .with_volume(sound.volume)
                .with_rate(sound.rate);
            self.api.play_sound(&params, 0);
        }
    }

    /// An owner over no cache: sounds can be requested and observed through
    /// [`Self::take_requests`] but have no audio to play.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn without_cache() -> Self {
        let loader = SoundLoader::new(None::<PackGroups>, true);
        let api = AudioApi::new(loader.clone(), Output::Clock, false, 0);
        Self {
            api,
            loader,
            prefs: VolumePreferences::default(),
            listener: None,
            camera: AdjustContext::default(),
            unknown: BTreeSet::new(),
            cutscene_sounds: std::collections::BTreeMap::new(),
            positioned: PositionedSounds::default(),
        }
    }

    /// Set the volumes the owner reads.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_volumes(&mut self, volumes: VolumePreferences) {
        self.prefs = volumes;
    }

    /// The sound requests the audio API received since the last call.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn take_requests(&mut self) -> Vec<SoundParams> {
        self.api.take_requests()
    }

    /// The id of the song currently playing, or -1.
    pub fn current_song(&self) -> i32 {
        self.api.current_song()
    }
}

/// The volume of a sequence sound. A sequence with a remote volume
/// percentage, played by an entity that can be targeted but is neither the
/// local player's target, the local player nor the active target, plays at
/// that percentage of its volume, clamped to 0-255.
fn sequence_sound_volume(sound: &SequenceSound, active_target: i32) -> i32 {
    if sound.target_volume == -1
        || sound.targeted == 0
        || sound.targeted == sound.local_targeted
        || active_target == sound.targeted
    {
        return sound.volume;
    }
    (sound.target_volume.wrapping_mul(sound.volume) / 100).clamp(0, 255)
}

/// Read the title song id from the `defaults` archive's audio file (file 4):
/// opcode 1 carries a big-endian u16 song id, opcode 0 ends the file.
fn title_song(pack: &Pack) -> anyhow::Result<Option<i32>> {
    let Some(bytes) = crate::js5_fetch::fetch_file(pack, "defaults", 4)? else {
        return Ok(None);
    };
    let mut song = None;
    let mut at = 0;
    loop {
        let Some(&opcode) = bytes.get(at) else {
            anyhow::bail!("defaults audio file without terminator");
        };
        at += 1;
        match opcode {
            0 => return Ok(song),
            1 => {
                let Some(pair) = bytes.get(at..at + 2) else {
                    anyhow::bail!("defaults audio file has a truncated title song");
                };
                song = Some(i32::from(u16::from_be_bytes([pair[0], pair[1]])));
                at += 2;
            }
            _ => {}
        }
    }
}

/// The sound and stream archives read the cache `Pack` (rs910-audio's
/// `GroupSource` seam). A local wrapper, because both
/// `Pack` (rs910-js5) and `GroupSource` (rs910-audio) are foreign here.
pub struct PackGroups(pub Pack);

impl rs910_audio::audio_stream::GroupSource for PackGroups {
    fn read_group(
        &self,
        archive_name: &str,
        group_id: u32,
    ) -> anyhow::Result<std::collections::BTreeMap<u32, Vec<u8>>> {
        Ok(Pack::read_group(&self.0, archive_name, group_id)?)
    }
}

impl Drop for AudioRuntime {
    fn drop(&mut self) {
        crate::audio_stream::stop_loader(&self.loader);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio_backend::{lock, RecordingSink};
    use crate::audio_stream::SoundState;
    use std::sync::Mutex;

    fn pack() -> Pack {
        crate::test_support::require_pack("client.vorbis.js5")
    }

    fn capture_api(pack: Pack) -> (AudioApi, Arc<Mutex<Vec<u8>>>) {
        let written = Arc::new(Mutex::new(Vec::new()));
        let lines: Vec<Box<dyn crate::audio_backend::Sink>> = (0..2)
            .map(|_| {
                Box::new(RecordingSink {
                    written: written.clone(),
                    capacity: 4096,
                }) as Box<dyn crate::audio_backend::Sink>
            })
            .collect();
        let loader = SoundLoader::new(Some(PackGroups(pack)), true);
        (
            AudioApi::new(loader, Output::Lines(lines), false, 0),
            written,
        )
    }

    const PREFS: VolumePreferences = VolumePreferences {
        sound: 255,
        background_sound: 255,
        speech: 255,
        music: 255,
        login_music: 255,
    };

    /// VORBIS_SOUND through the whole path: play request -> sound -> voice ->
    /// vorbis decoder -> sample ring -> line mixer.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn vorbis_sound_plays_to_the_player_line_and_finishes() {
        let pack = pack();
        let (mut api, written) = capture_api(pack);
        let camera = AdjustContext::default();
        api.play_sound(
            &SoundParams::new(SoundType::Effect, 1, sub_buss_type::SFX_SUB),
            0,
        );
        let key = api.active_sounds()[0];
        let mut now = 0_i64;
        let mut playing = false;
        for _ in 0..4000 {
            now += 20;
            api.update(now, &PREFS, &camera, None);
            api.backend().step_decode();
            api.backend().step_output();
            playing |= api.sound_state(key) == SoundState::Playing;
            if api.active_sounds().is_empty() {
                break;
            }
        }
        assert!(playing, "sound reached the playing state");
        assert!(
            api.active_sounds().is_empty(),
            "finished sound left the active sounds"
        );
        assert_eq!(api.sound_state(key), SoundState::Released);
        let bytes = lock(&written);
        let samples: Vec<i16> = bytes
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        // vorbis group 1: 76737 mono frames at 22050 Hz, played on the
        // 22.05 kHz player as stereo (mono upmixed).
        assert!(
            samples.len() >= 76737 * 2,
            "{} samples mixed",
            samples.len()
        );
        let peak = samples
            .iter()
            .map(|s| i32::from(*s).abs())
            .max()
            .unwrap_or(0);
        // SFX buss priority does not scale gain; buss volume 1.0 * sound
        // volume 1.0, squared.
        assert!(peak > 10000, "peak {peak}");
    }

    /// MIDI_SONG streams its JAGA chunk groups from `audiostreams`, reports
    /// SOUND_SONGPRELOADED on start and SOUND_SONGEND when it ends.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn song_streams_chunks_and_reports_song_end() {
        let pack = pack();
        let (mut api, written) = capture_api(pack);
        api.in_game = true;
        let camera = AdjustContext::default();
        api.play_song(2, 255, None);
        assert_eq!(api.current_song(), 2);
        assert_eq!(api.outgoing, [Outgoing::SongPreloaded(2)]);
        api.outgoing.clear();
        let mut now = 0_i64;
        for _ in 0..20000 {
            now += 20;
            api.update(now, &PREFS, &camera, None);
            api.backend().step_decode();
            api.backend().step_output();
            if !api.outgoing.is_empty() {
                break;
            }
        }
        assert_eq!(api.outgoing, [Outgoing::SongEnd(2)]);
        assert_eq!(api.current_song(), -1);
        // audiostreams 2: 176640 stereo frames over two chunk groups.
        let frames = lock(&written).len() / 4;
        assert!(frames >= 176640, "{frames} frames mixed");
    }

    /// The original client's sequence-sound entry, recorded over 280 cases
    /// (`fixtures/recorded/audio-sequence-sound`): player, NPC, spot animation
    /// and projectile emitters against the local player's and the active
    /// target, level, volume preferences and remote volume percentages. Each
    /// case runs through the game's gate and row resolution and then the
    /// audio owner; the sound request it makes (or none) must be the one the
    /// original makes.
    #[test]
    fn sequence_sounds_match_the_original_client() {
        use rs910_core::test_support::frozen;
        use rs910_game::protocol910::sequence_types::Sequence;
        use rs910_game::sequence_sound::{emit, Emitter};
        let cases = frozen::text("audio-sequence-sound/cases.tsv");
        let expected = frozen::text("audio-sequence-sound/expected.tsv");
        let cases: Vec<&str> = cases.lines().filter(|l| !l.starts_with('#')).collect();
        let expected: Vec<&str> = expected.lines().collect();
        assert_eq!(cases.len(), expected.len());
        let mut audio = AudioRuntime::without_cache();
        let mut rng = rs910_game::animation_playback::AnimationRandom::new(1);
        let mut plays = 0;
        for (case, want) in cases.iter().zip(&expected) {
            let f: Vec<&str> = case.split('\t').collect();
            let int = |i: usize| f[i].parse::<i32>().unwrap();
            let float = |i: usize| f[i].parse::<f32>().unwrap();
            let (own, local_index, active) = (int(1), int(2), int(3));
            let mut seq = Sequence::empty(1);
            seq.sound = Some(vec![Some(vec![int(4)])]);
            seq.volume = (int(5) >= 0).then(|| vec![int(5)]);
            seq.remote_volume_percent = int(6);
            if int(13) >= 0 {
                seq.minrate = Some(vec![int(13)]);
                seq.maxrate = Some(vec![int(14)]);
            }
            let emitter = Emitter {
                level: int(9),
                fine_x: float(11),
                fine_z: float(12),
                local: f[15] == "1",
                visible: true,
                targeted: match f[0] {
                    "player" => -own - 1,
                    "npc" => own + 1,
                    _ => own,
                },
                listener: Some((int(10), -local_index - 1)),
            };
            audio.set_volumes(VolumePreferences {
                sound: int(7),
                background_sound: int(8),
                ..PREFS
            });
            let mut sounds = Vec::new();
            emit(&seq, 0, emitter, &mut rng, &mut sounds);
            for sound in &sounds {
                audio.play_sequence_sound(sound, active);
            }
            let got = audio
                .take_requests()
                .iter()
                .map(|r| {
                    let kind = match r.kind {
                        SoundType::SequenceArea => "area",
                        _ => "local",
                    };
                    let shape = match r.shape {
                        SoundShape::None => "none",
                        _ => "stereo",
                    };
                    let (x, z) = r.position.map_or(("-".to_string(), "-".to_string()), |p| {
                        ((p[0] as i32).to_string(), (p[2] as i32).to_string())
                    });
                    format!(
                        "{kind}\t{}\t{}\t{}\t{}\t{shape}\t{}\t{}\t{x}\t{z}\t{}",
                        r.id, r.loops, r.volume, r.bus, r.size as i32, r.range as i32, r.rate
                    )
                })
                .next()
                .unwrap_or_else(|| "none".to_string());
            assert_eq!(&got, want, "case {case}");
            plays += usize::from(got != "none");
        }
        assert!(plays > 100, "{plays} cases play a sound");
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn requests_route_to_the_audio_api() {
        let pack = pack();
        let mut runtime = AudioRuntime::new(pack);
        let t0 = crate::logic_clock::monotonic_millis();
        let mut out = Vec::new();
        runtime.tick(t0, true, &mut out);
        // SOUND_MIXBUSS_ADD / SETLEVEL reach the buss tree.
        runtime.submit(&SoundRequest {
            command: "sound_mixbuss_add".into(),
            args: vec![77, -1, 32768],
        });
        runtime.submit(&SoundRequest {
            command: "sound_mixbuss_setlevel".into(),
            args: vec![77, 16384],
        });
        runtime.tick(t0 + 50, true, &mut out);
        runtime.tick(t0 + 300, true, &mut out);
        assert!((runtime.api.mix_buss_volume(77) - 0.25).abs() < 1e-6);
        // MIDI_SONG queues SOUND_SONGPRELOADED p4(song) when in game.
        runtime.submit(&SoundRequest {
            command: "midi_song".into(),
            args: vec![2, 255],
        });
        runtime.tick(t0 + 320, true, &mut out);
        assert_eq!(out, [crate::proto::client::SOUND_SONGPRELOADED, 0, 0, 0, 2]);
        assert_eq!(runtime.current_song(), 2);
        // MIDI_SONG_STOP sends SOUND_SONGEND for the current song.
        out.clear();
        runtime.submit(&SoundRequest {
            command: "midi_song_stop".into(),
            args: vec![],
        });
        runtime.tick(t0 + 340, true, &mut out);
        assert_eq!(out, [crate::proto::client::SOUND_SONGEND, 0, 0, 0, 2]);
        assert_eq!(runtime.current_song(), -1);
    }
}
