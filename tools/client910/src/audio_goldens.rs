//! Sample-level goldens of the audio stack.
//!
//! The FNV-1a 64 digest of the bytes the two line mixers write to their
//! lines while `AudioApi` plays a mix of effects (plain, resampled,
//! stereo-positioned) and a streamed song, with bus eases and a camera turn.
//! The real-cache Vorbis decodes are checked against an independent
//! reference decoder in `mod vorbis`. The whole-cache regression digest is in
//! `audio_corpus`. These read `server/data/pack`, so they stay in client910.

use crate::audio_api::{AdjustContext, AudioApi, SoundParams, SoundShape};
use crate::audio_backend::{sub_buss_type, Output, Sink, VolumePreferences};
use crate::audio_stream::{SoundLoader, SoundType};
use std::sync::{Arc, Mutex};

/// FNV-1a 64.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn pack() -> crate::cache::Pack {
    crate::test_support::require_pack("client.vorbis.js5")
}

/// A line that accepts `capacity` bytes per refresh and records them.
struct Recorder {
    written: Arc<Mutex<Vec<u8>>>,
    capacity: i32,
}

impl Sink for Recorder {
    fn available(&mut self) -> i32 {
        self.capacity
    }

    fn write(&mut self, bytes: &[u8]) {
        self.written.lock().unwrap().extend_from_slice(bytes);
    }
}

/// `AudioApi` -> `Sound` -> voices -> sample rings -> both line mixers,
/// stepped deterministically (no worker threads, synchronous loading, a
/// fixed 20 ms clock): the bytes each player writes to its line.
///
/// The digest is a self-recorded snapshot: update it only for an intended
/// mixer or decoder change, from the digests the failure prints.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn mixed_player_lines_match_golden() {
    let written: Vec<Arc<Mutex<Vec<u8>>>> = (0..2).map(|_| Arc::default()).collect();
    let lines = written
        .iter()
        .map(|w| {
            Box::new(Recorder {
                written: w.clone(),
                capacity: 4096,
            }) as Box<dyn Sink>
        })
        .collect();
    let loader = SoundLoader::new(Some(crate::audio_runtime::PackGroups(pack())), true);
    let mut api = AudioApi::new(loader, Output::Lines(lines), false, 0);
    let prefs = VolumePreferences {
        sound: 200,
        background_sound: 255,
        speech: 255,
        music: 180,
        login_music: 255,
    };
    let mut camera = AdjustContext::default();
    let listener = Some([0.0, 0.0, 0.0]);
    let mut now = 0_i64;
    for cycle in 0..900 {
        now += 20;
        match cycle {
            // Positioned sounds need a listener, which the first update sets.
            1 => {
                // Unpositioned effect at the 22.05 kHz player's own rate.
                api.play_sound(
                    &SoundParams {
                        kind: SoundType::Effect,
                        id: 1,
                        loops: 1,
                        volume: 255,
                        bus: sub_buss_type::SFX_SUB,
                        shape: SoundShape::None,
                        size: 0.0,
                        range: 0.0,
                        position: None,
                        rate: 255,
                    },
                    0,
                );
                // Streamed song on MUSIC_SUB.
                api.play_song(2, 200, None);
            }
            // Resampled (rate 300/255) stereo-positioned effect, delayed.
            40 => api.play_sound(
                &SoundParams {
                    kind: SoundType::Effect,
                    id: 1,
                    loops: 2,
                    volume: 220,
                    bus: sub_buss_type::SFX_SUB,
                    shape: SoundShape::Stereo,
                    size: 0.0,
                    range: 8192.0,
                    position: Some([2048.0, 0.0, 1024.0]),
                    rate: 300,
                },
                5,
            ),
            // A camera turn re-pans the positioned voice.
            120 => camera.camera_yaw = 4096,
            // A provider-less bus ease.
            200 => api.set_mix_buss_level(sub_buss_type::SFX_SUB, 16384),
            // A higher-rate effect lands on the 44.1 kHz player.
            260 => api.play_sound(
                &SoundParams {
                    kind: SoundType::Effect,
                    id: 1,
                    loops: 1,
                    volume: 180,
                    bus: sub_buss_type::SFX_SUB,
                    shape: SoundShape::None,
                    size: 0.0,
                    range: 0.0,
                    position: None,
                    rate: 600,
                },
                0,
            ),
            400 => api.stop_song(),
            _ => {}
        }
        api.update(now, &prefs, &camera, listener);
        api.backend().step_decode();
        api.backend().step_output();
    }
    let digests: Vec<(usize, u64)> = written
        .iter()
        .map(|w| {
            let bytes = w.lock().unwrap();
            (bytes.len(), fnv1a(&bytes))
        })
        .collect();
    assert_eq!(
        digests,
        [
            (260_828, 0x6b6e_0640_a105_de81),
            (706_560, 0x7d50_2efa_9ee3_0d92)
        ],
        "{digests:#x?}"
    );
}

/// Real-cache decodes of the Vorbis decoder against an independent
/// reference (ffmpeg) and its loop handling.
mod vorbis {
    use rs910_audio::audio_sound_file::SoundFile;
    use rs910_audio::audio_vorbis::{DecoderState, Endianness, SampleFormat, VorbisDecoder};

    fn pack() -> crate::cache::Pack {
        crate::test_support::require_pack("client.vorbis.js5")
    }

    fn decoder() -> VorbisDecoder {
        let mut decoder = VorbisDecoder::new(2.0);
        decoder.configure(SampleFormat::Signed16, Endianness::Little, 2);
        decoder
    }

    /// Decode until the decoder stops producing, as the voice tasks would.
    fn drain(decoder: &mut VorbisDecoder, pcm: &mut Vec<i16>) {
        loop {
            decoder.decode_tick().unwrap();
            let before = pcm.len();
            if decoder.ready() {
                let chunk = decoder.take_pcm(1 << 16).unwrap();
                pcm.extend(
                    chunk
                        .bytes
                        .chunks(2)
                        .map(|b| i16::from_le_bytes([b[0], b[1]])),
                );
            }
            if pcm.len() == before && decoder.state() != DecoderState::Ready {
                break;
            }
        }
    }

    fn feed(decoder: &mut VorbisDecoder, bytes: Vec<u8>, pcm: &mut Vec<i16>) {
        decoder.push_chunk(bytes);
        drain(decoder, pcm);
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn decodes_real_vorbis_group() {
        let pack = pack();
        let bytes = pack.read_group("vorbis", 1).unwrap().remove(&0).unwrap();
        let header = SoundFile::parse(&bytes).unwrap();
        assert_eq!(
            (header.sample_rate, header.channels, header.chunks.len()),
            (22050, 1, 1)
        );
        let mut sound = decoder();
        let mut pcm = Vec::new();
        for chunk in &header.chunks {
            feed(
                &mut sound,
                bytes[chunk.offset..chunk.offset + chunk.size].to_vec(),
                &mut pcm,
            );
        }
        sound.push_end_of_input();
        drain(&mut sound, &mut pcm);
        assert_eq!(sound.state(), DecoderState::Ended);
        assert_eq!(sound.sample_rate().unwrap(), 22050);
        // Mono is upmixed to two output channels, and the last page trims to
        // its granule position, which the loop end also records.
        assert_eq!(pcm.len(), header.loop_end as usize * 2);
        assert!(pcm.chunks(2).all(|f| f[0] == f[1]));
        // Independent reference: `ffmpeg -i <group 1 ogg> -ac 1 -f s16le`.
        // The client truncates to 16 bits where ffmpeg rounds, so allow 2 LSB.
        for (at, expected) in [
            (
                5000,
                [-5383, -9366, -9625, -5964, -5573, -9835, -5487, -3697],
            ),
            (
                40000,
                [-2221, -2339, -2426, -3047, -3263, -3573, -2968, -1920],
            ),
        ] {
            for (i, value) in expected.iter().enumerate() {
                let got = i32::from(pcm[(at + i) * 2]);
                assert!(
                    (got - value).abs() <= 2,
                    "frame {}: {got} vs {value}",
                    at + i
                );
            }
        }
    }

    /// An `audiostreams` song: chunk 0 is inline after the header table and
    /// later chunks are separate groups. Stereo streams use residue type 2,
    /// which the client decodes over a working buffer it does not clear
    /// first, so the output differs slightly from a reference decoder there.
    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn decodes_real_audiostreams_song() {
        let pack = pack();
        let bytes = pack
            .read_group("audiostreams", 2)
            .unwrap()
            .remove(&0)
            .unwrap();
        let header = SoundFile::parse(&bytes).unwrap();
        assert_eq!(
            (header.sample_rate, header.channels, header.chunks.len()),
            (22050, 2, 2)
        );
        let first = header.chunks[0];
        assert_eq!(first.offset + first.size, bytes.len());
        let mut sound = decoder();
        let mut pcm = Vec::new();
        feed(&mut sound, bytes[header.table_end..].to_vec(), &mut pcm);
        let chunk = pack
            .read_group("audiostreams", header.chunks[1].group as u32)
            .unwrap()
            .remove(&0)
            .unwrap();
        feed(&mut sound, chunk, &mut pcm);
        sound.push_end_of_input();
        drain(&mut sound, &mut pcm);
        assert_eq!(sound.state(), DecoderState::Ended);
        assert_eq!(pcm.len(), header.loop_end as usize * 2);
        // ffmpeg reference for left frames 1000..1004 (see note above).
        for (i, value) in [-9759, -7817, -5000, -2879].iter().enumerate() {
            let got = i32::from(pcm[(1000 + i) * 2]);
            assert!(
                (got - value).abs() <= 64,
                "frame {}: {got} vs {value}",
                1000 + i
            );
        }
    }

    #[test]
    #[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
    fn loop_points_restart_the_stream() {
        let pack = pack();
        let bytes = pack.read_group("vorbis", 1).unwrap().remove(&0).unwrap();
        let header = SoundFile::parse(&bytes).unwrap();
        let chunk = header.chunks[0];
        let mut sound = decoder();
        // Two extra loops over the whole sound.
        sound.set_loop(true, 2, 0, header.loop_end);
        let mut pcm = Vec::new();
        for _ in 0..3 {
            feed(
                &mut sound,
                bytes[chunk.offset..chunk.offset + chunk.size].to_vec(),
                &mut pcm,
            );
            sound.push_end_of_input();
        }
        drain(&mut sound, &mut pcm);
        assert_eq!(sound.state(), DecoderState::Ended);
        let frames = pcm.len() / 2;
        let one = header.loop_end as usize;
        assert!(
            frames >= one * 3 - 3 && frames <= one * 3 + 3,
            "{frames} frames for 3 passes of {one}"
        );
    }
}
