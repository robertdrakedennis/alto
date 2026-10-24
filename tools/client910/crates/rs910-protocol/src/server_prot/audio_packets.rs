//! Sound and song packet decoding into retained events.

/// `PayloadReader` and `cp1252_byte` (split out in Phase 2.2).
pub use crate::payload_reader::*;

use super::UiEvent;

pub(super) fn parse_audio_event(opcode: u8, payload: &[u8]) -> anyhow::Result<Option<UiEvent>> {
    use crate::proto::server as p;

    let (command, args) = match opcode {
        p::MIDI_JINGLE => {
            anyhow::ensure!(payload.len() == 3, "MIDI_JINGLE length");
            let mut r = PayloadReader::new(payload);
            let volume = i32::from(r.g1_alt1()?);
            let raw_song = r.g2_alt1()?;
            r.finish("MIDI_JINGLE")?;
            (
                "midi_jingle",
                vec![
                    if raw_song == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_song)
                    },
                    volume,
                ],
            )
        }
        p::MIDI_SONG => {
            anyhow::ensure!(payload.len() == 3, "MIDI_SONG length");
            let mut r = PayloadReader::new(payload);
            let raw_song = r.g2_alt3()?;
            let volume = i32::from(r.g1_alt2()?);
            r.finish("MIDI_SONG")?;
            (
                "midi_song",
                vec![
                    if raw_song == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_song)
                    },
                    volume,
                ],
            )
        }
        p::MIDI_SONG_STOP => {
            anyhow::ensure!(payload.is_empty(), "MIDI_SONG_STOP length");
            ("midi_song_stop", Vec::new())
        }
        p::MIDI_SONG_LOCATION => {
            anyhow::ensure!(payload.len() == 11, "MIDI_SONG_LOCATION length");
            let mut r = PayloadReader::new(payload);
            // The level shift is a signed shift with no mask; x/z stay
            // absolute tiles and the sound creation compares them (<< 9)
            // against the scene-local listener position, exactly as the
            // reference client does.
            let packed = r.g4_alt3()?;
            let raw_song = r.g4s()?;
            let size = i32::from(r.g1_alt1()?);
            let volume = i32::from(r.g1()?);
            let range = i32::from(r.g1_alt2()?);
            r.finish("MIDI_SONG_LOCATION")?;
            let level = packed >> 28;
            let x = (packed >> 14) & 0x3fff;
            let z = packed & 0x3fff;
            (
                "midi_song_location",
                vec![raw_song, volume, level, x, z, size, range],
            )
        }
        p::SYNTH_SOUND => {
            anyhow::ensure!(payload.len() == 8, "SYNTH_SOUND length");
            let mut r = PayloadReader::new(payload);
            let raw_sound = r.g2()?;
            let loops = i32::from(r.g1()?);
            let delay = i32::from(r.g2()?);
            let volume = i32::from(r.g1()?);
            let rate = i32::from(r.g2()?);
            r.finish("SYNTH_SOUND")?;
            (
                "synth_sound",
                vec![
                    if raw_sound == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_sound)
                    },
                    loops,
                    delay,
                    volume,
                    rate,
                ],
            )
        }
        p::VORBIS_SOUND => {
            anyhow::ensure!(payload.len() == 8, "VORBIS_SOUND length");
            let mut r = PayloadReader::new(payload);
            let raw_sound = r.g2()?;
            let loops = i32::from(r.g1()?);
            let delay = i32::from(r.g2()?);
            let volume = i32::from(r.g1()?);
            let rate = i32::from(r.g2()?);
            r.finish("VORBIS_SOUND")?;
            (
                "vorbis_sound",
                vec![
                    if raw_sound == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_sound)
                    },
                    loops,
                    delay,
                    volume,
                    rate,
                ],
            )
        }
        p::VORBIS_SPEECH_SOUND => {
            anyhow::ensure!(payload.len() == 6, "VORBIS_SPEECH_SOUND length");
            let mut r = PayloadReader::new(payload);
            let raw_sound = r.g2()?;
            let loops = i32::from(r.g1()?);
            let delay = i32::from(r.g2()?);
            let volume = i32::from(r.g1()?);
            r.finish("VORBIS_SPEECH_SOUND")?;
            (
                "vorbis_speech_sound",
                vec![
                    if raw_sound == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_sound)
                    },
                    loops,
                    delay,
                    volume,
                ],
            )
        }
        p::VORBIS_SPEECH_STOP => {
            anyhow::ensure!(payload.is_empty(), "VORBIS_SPEECH_STOP length");
            ("vorbis_speech_stop", Vec::new())
        }
        p::SONG_PRELOAD => {
            anyhow::ensure!(payload.len() == 2, "SONG_PRELOAD length");
            let mut r = PayloadReader::new(payload);
            let raw_song = r.g2_alt1()?;
            r.finish("SONG_PRELOAD")?;
            (
                "song_preload",
                vec![
                    if raw_song == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_song)
                    },
                    255,
                ],
            )
        }
        p::VORBIS_PRELOAD_SOUNDS => {
            anyhow::ensure!(payload.len() == 2, "VORBIS_PRELOAD_SOUNDS length");
            let mut r = PayloadReader::new(payload);
            let group = i32::from(r.g2()?);
            r.finish("VORBIS_PRELOAD_SOUNDS")?;
            ("vorbis_preload_sounds", vec![group])
        }
        p::VORBIS_PRELOAD_SOUND_GROUP => {
            anyhow::ensure!(payload.len() == 2, "VORBIS_PRELOAD_SOUND_GROUP length");
            let mut r = PayloadReader::new(payload);
            let group = i32::from(r.g2()?);
            r.finish("VORBIS_PRELOAD_SOUND_GROUP")?;
            ("vorbis_preload_sound_group", vec![group])
        }
        p::VORBIS_SOUND_GROUP_START | p::VORBIS_SOUND_GROUP_STOP => {
            anyhow::ensure!(payload.len() == 2, "VORBIS_SOUND_GROUP control length");
            let mut r = PayloadReader::new(payload);
            let group = i32::from(r.g2()?);
            r.finish("VORBIS_SOUND_GROUP control")?;
            let command = if opcode == p::VORBIS_SOUND_GROUP_START {
                "vorbis_sound_group_start"
            } else {
                "vorbis_sound_group_stop"
            };
            (command, vec![group])
        }
        p::VORBIS_SOUND_GROUP => {
            anyhow::ensure!(payload.len() == 10, "VORBIS_SOUND_GROUP length");
            let mut r = PayloadReader::new(payload);
            let raw_sound = r.g2()?;
            let loops = i32::from(r.g1()?);
            let delay = i32::from(r.g2()?);
            let volume = i32::from(r.g1()?);
            let rate = i32::from(r.g2()?);
            let group = i32::from(r.g2()?);
            r.finish("VORBIS_SOUND_GROUP")?;
            (
                "vorbis_sound_group",
                vec![
                    if raw_sound == u16::MAX {
                        -1
                    } else {
                        i32::from(raw_sound)
                    },
                    loops,
                    delay,
                    volume,
                    rate,
                    group,
                ],
            )
        }
        p::SOUND_MIXBUSS_ADD => {
            anyhow::ensure!(payload.len() == 6, "SOUND_MIXBUSS_ADD length");
            let mut r = PayloadReader::new(payload);
            let bus = i32::from(r.g2()?);
            let parent = i32::from(r.g2()?);
            let level = i32::from(r.g2()?);
            r.finish("SOUND_MIXBUSS_ADD")?;
            ("sound_mixbuss_add", vec![bus, parent, level])
        }
        p::SOUND_MIXBUSS_SETLEVEL => {
            anyhow::ensure!(payload.len() == 4, "SOUND_MIXBUSS_SETLEVEL length");
            let mut r = PayloadReader::new(payload);
            let bus = i32::from(r.g2()?);
            let level = i32::from(r.g2()?);
            r.finish("SOUND_MIXBUSS_SETLEVEL")?;
            ("sound_mixbuss_setlevel", vec![bus, level])
        }
        _ => return Ok(None),
    };
    Ok(Some(UiEvent::Audio {
        command: command.into(),
        args,
    }))
}
