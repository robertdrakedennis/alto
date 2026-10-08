//! rs910-audio: the audio stack of the client.
//!
//! Modules, bottom up (no module depends on one above it):
//!
//! 1. [`audio_vorbis`] is the Ogg/Vorbis decoder every sound goes through.
//! 2. [`audio_sound_file`] parses the small header in front of each sound's
//!    Ogg stream (loop points, format, chunk table).
//! 3. [`audio_sink`] holds the [`audio_sink::Sink`] boundary trait with the
//!    `cpal` device, clock and recording sinks.
//! 4. [`audio_bus`] holds the bus tree and its volume providers.
//! 5. [`audio_mixer`] holds the line mixers and their sample rings.
//! 6. [`audio_adjust`] holds the positional gain adjusters.
//! 7. [`audio_voice`] holds one voice of the pool.
//! 8. [`audio_backend`] holds the voice pool and its worker threads.
//! 9. [`audio_stream`] holds group loading, streams and sounds.
//! 10. [`audio_api`] holds the audio owner: songs, jingles, groups and the
//!     sound requests.
//!
//! The client-side owner (`audio_runtime`) and positioned sounds
//! (`positioned_sound`) live in the UI crate, which routes script and packet
//! requests here.

pub mod audio_adjust;
pub mod audio_api;
pub mod audio_backend;
pub mod audio_bus;
pub mod audio_mixer;
pub mod audio_sink;
pub mod audio_sound_file;
pub mod audio_stream;
pub mod audio_voice;
pub mod audio_vorbis;
