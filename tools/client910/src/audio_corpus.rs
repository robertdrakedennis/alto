//! The audio regression proof: sound groups of the local cache decoded with
//! the production decoder and compared with a committed digest.
//!
//! `fixtures/audio/corpus-digest.tsv` covers every group of the `vorbis`
//! archive (effects, whole groups) and of the `audiostreams` archive (songs,
//! chunked). Each section holds one row per group:
//!
//! - `sound`: output frames and the low 48 bits of the FNV-1a 64 hash of the
//!   decoded 16-bit stereo PCM;
//! - `loop`: the same after a loop restart (two passes over the header's
//!   loop points), for every 16th group.
//!
//! The digest holds hashes and counts only, never audio content.
//!
//! Decoding the whole cache takes about ten CPU-minutes, so
//! [`corpus_matches_committed_digest`] is `#[ignore]`d (run it before a
//! release or after a decoder change) and [`sampled_corpus_matches`] checks a
//! spread of groups in the default suite: every 100th effect, every stereo or
//! unusual-rate effect, every mono song and every 250th audiostreams group.
//!
//! The cache is local-only (never committed), so these tests need
//! `server/data/pack`. Record the digest again only for an intended decoder
//! change: `cargo test -p client910 --lib record_audio_corpus -- --ignored`.

use rs910_audio::audio_sound_file::SoundFile;
use rs910_audio::audio_vorbis::{DecoderState, Endianness, SampleFormat, VorbisDecoder};
use rs910_js5::cache::Pack;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

const ARCHIVES: [&str; 2] = ["vorbis", "audiostreams"];

/// Every Nth group is decoded a second time through a loop restart.
const LOOP_STRIDE: usize = 16;

/// Sampled run: every Nth effect and every Nth audiostreams group.
const EFFECT_SAMPLE: usize = 100;
const STREAM_SAMPLE: usize = 250;

const HASH_MASK: u64 = 0xffff_ffff_ffff;

fn fixture_path() -> PathBuf {
    rs910_core::test_support::client_dir().join("fixtures/audio/corpus-digest.tsv")
}

fn fnv1a(hash: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *hash ^= u64::from(b);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

/// One decoded sound.
struct Decoded {
    frames: usize,
    hash: u64,
}

impl Decoded {
    fn digest(&self) -> String {
        format!("{}\t{:012x}", self.frames, self.hash & HASH_MASK)
    }
}

/// Decode a whole sound the way its voice would: each chunk in order, then
/// the end of input, restarting for `extra_loops` more passes. Every run of
/// output PCM bytes goes to `sink`.
fn decode_with(
    loop_points: (i32, i32),
    chunks: &[Vec<u8>],
    extra_loops: i32,
    mut sink: impl FnMut(&[u8]),
) -> usize {
    let mut decoder = VorbisDecoder::new(2.0);
    decoder.configure(SampleFormat::Signed16, Endianness::Little, 2);
    if extra_loops > 0 {
        decoder.set_loop(true, extra_loops, loop_points.0, loop_points.1);
    }
    let mut frames = 0;
    let mut drain = |decoder: &mut VorbisDecoder| loop {
        decoder.decode_tick().expect("decode");
        let before = frames;
        if decoder.ready() {
            let bytes = decoder.take_pcm(1 << 16).expect("take pcm").bytes;
            sink(&bytes);
            frames += bytes.len() / 4;
        }
        if frames == before && decoder.state() != DecoderState::Ready {
            break;
        }
    };
    for _ in 0..=extra_loops {
        for chunk in chunks {
            decoder.push_chunk(chunk.clone());
            drain(&mut decoder);
        }
        decoder.push_end_of_input();
        drain(&mut decoder);
    }
    assert_eq!(decoder.state(), DecoderState::Ended);
    frames
}

/// The frame count and PCM hash of a sound (see [`decode_with`]).
fn decode(sound: &SoundFile, chunks: &[Vec<u8>], extra_loops: i32) -> Decoded {
    let mut hash = 0xcbf2_9ce4_8422_2325;
    let frames = decode_with(
        (sound.loop_start, sound.loop_end),
        chunks,
        extra_loops,
        |bytes| fnv1a(&mut hash, bytes),
    );
    Decoded { frames, hash }
}

/// The chunks of a sound: inline after the header table for an effect; for a
/// song the first chunk inline and the rest fetched from their own groups.
fn chunks_of(pack: &Pack, archive: &str, bytes: &[u8], sound: &SoundFile) -> Option<Vec<Vec<u8>>> {
    let mut chunks = Vec::new();
    if archive == "vorbis" {
        for chunk in &sound.chunks {
            chunks.push(bytes[chunk.offset..chunk.offset + chunk.size].to_vec());
        }
    } else {
        chunks.push(bytes[sound.table_end..].to_vec());
        for chunk in sound.chunks.iter().skip(1) {
            let mut files = pack.read_group(archive, chunk.group as u32).ok()?;
            chunks.push(files.remove(&0)?);
        }
    }
    Some(chunks)
}

struct Row {
    archive: &'static str,
    group: u32,
    plain: Decoded,
    looped: Option<Decoded>,
}

/// Which groups a run covers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Coverage {
    All,
    Sampled,
}

/// Whether the sampled run decodes group number `index` of the combined
/// list of both archives.
fn in_sample(archive: &str, index: usize, sound: &SoundFile) -> bool {
    if archive == "vorbis" {
        index.is_multiple_of(EFFECT_SAMPLE) || sound.channels != 1 || sound.sample_rate == 32000
    } else {
        sound.channels == 1 || index.is_multiple_of(STREAM_SAMPLE)
    }
}

fn run_corpus(pack: &Pack, coverage: Coverage) -> Vec<Row> {
    let mut groups = Vec::new();
    for archive in ARCHIVES {
        let index = pack.read_archive_index(archive).expect("archive index");
        groups.extend(index.group_id.iter().map(|&g| (archive, g)));
    }
    let next = AtomicUsize::new(0);
    let rows: Mutex<BTreeMap<usize, Row>> = Mutex::new(BTreeMap::new());
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get().min(8));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&(archive, group)) = groups.get(i) else {
                    break;
                };
                let Some(bytes) = pack
                    .read_group(archive, group)
                    .ok()
                    .and_then(|mut files| files.remove(&0))
                else {
                    continue;
                };
                // Chunk groups of songs are not sounds; they fail to parse.
                let Ok(sound) = SoundFile::parse(&bytes) else {
                    continue;
                };
                if coverage == Coverage::Sampled && !in_sample(archive, i, &sound) {
                    continue;
                }
                let Some(chunks) = chunks_of(pack, archive, &bytes, &sound) else {
                    continue;
                };
                let plain = decode(&sound, &chunks, 0);
                let looped = i
                    .is_multiple_of(LOOP_STRIDE)
                    .then(|| decode(&sound, &chunks, 1));
                rows.lock().unwrap().insert(
                    i,
                    Row {
                        archive,
                        group,
                        plain,
                        looped,
                    },
                );
            });
        }
    });
    rows.into_inner().unwrap().into_values().collect()
}

/// The rows of every section, keyed by `(section, archive, group)`.
type Table = BTreeMap<(String, String, u32), String>;

fn table(rows: &[Row]) -> Table {
    let mut table = Table::new();
    for row in rows {
        let key = |section: &str| (section.to_string(), row.archive.to_string(), row.group);
        table.insert(key("sound"), row.plain.digest());
        if let Some(looped) = &row.looped {
            table.insert(key("loop"), looped.digest());
        }
    }
    table
}

fn render(table: &Table) -> String {
    let mut text = String::from(
        "# Audio corpus digest: see src/audio_corpus.rs. Sections: sound (decoded once), loop (two passes over the loop points).\n\
         # Rows: group, output frames (stereo), low 48 bits of the FNV-1a 64 hash of the PCM bytes. Hashes and counts only.\n",
    );
    for section in ["sound", "loop"] {
        for archive in ARCHIVES {
            let _ = writeln!(text, "@ {section} {archive}");
            for ((s, a, group), value) in table {
                if s == section && a == archive {
                    let _ = writeln!(text, "{group}\t{value}");
                }
            }
        }
    }
    text
}

fn parse(text: &str) -> Table {
    let mut table = Table::new();
    let mut section = (String::new(), String::new());
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if let Some(header) = line.strip_prefix("@ ") {
            let (name, archive) = header.rsplit_once(' ').expect("section header");
            section = (name.to_string(), archive.to_string());
            continue;
        }
        let (group, value) = line.split_once('\t').expect("row");
        table.insert(
            (
                section.0.clone(),
                section.1.clone(),
                group.parse().expect("group"),
            ),
            value.to_string(),
        );
    }
    table
}

#[test]
#[ignore = "records fixtures/audio/corpus-digest.tsv from the local cache (about 10 CPU-minutes)"]
fn record_audio_corpus() {
    let pack = crate::test_support::require_pack("client.vorbis.js5");
    let rows = run_corpus(&pack, Coverage::All);
    std::fs::create_dir_all(fixture_path().parent().unwrap()).unwrap();
    std::fs::write(fixture_path(), render(&table(&rows))).unwrap();
}

/// Compare a run with the committed digest: every decoded row must exist in
/// the digest with the same value, and a whole-cache run must also leave
/// nothing in the digest undecoded.
fn check(coverage: Coverage) {
    let pack = crate::test_support::require_pack("client.vorbis.js5");
    let now = table(&run_corpus(&pack, coverage));
    let expected = parse(&std::fs::read_to_string(fixture_path()).expect("corpus-digest.tsv"));
    let mut bad: Vec<String> = Vec::new();
    for (key, value) in &now {
        match expected.get(key) {
            Some(want) if want == value => {}
            Some(want) => bad.push(format!("{key:?}: decoded {value}, digest {want}")),
            None => bad.push(format!("{key:?}: decoded {value}, not in the digest")),
        }
    }
    if coverage == Coverage::All {
        for key in expected.keys().filter(|k| !now.contains_key(*k)) {
            bad.push(format!("{key:?}: in the digest, not decoded"));
        }
    }
    assert!(
        bad.is_empty(),
        "{} of {} rows differ, first 20:\n{}",
        bad.len(),
        now.len(),
        bad.iter().take(20).cloned().collect::<Vec<_>>().join("\n")
    );
    assert!(!now.is_empty(), "no groups decoded");
}

#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn sampled_corpus_matches() {
    check(Coverage::Sampled);
}

#[test]
#[ignore = "nightly: decodes the whole cache (about 10 CPU-minutes)"]
fn corpus_matches_committed_digest() {
    check(Coverage::All);
}

/// Two sounds whose PCM the original client's decoder produced (recorded
/// once, block digests in `fixtures/recorded/audio-vorbis/`): a stereo effect
/// of fourteen chained Ogg streams and a stereo song. Both are residue type 2
/// with channel coupling, and both differ from a general Vorbis decoder in the
/// ways the module notes describe (see `rs910_audio::audio_vorbis`), so they
/// pin the client's own output where a reference library would disagree.
#[test]
#[cfg_attr(feature = "no-pack", ignore = "needs server/data/pack")]
fn stereo_sounds_match_the_original_decoder() {
    let pack = crate::test_support::require_pack("client.vorbis.js5");
    for (archive, group, name) in [
        ("vorbis", 356, "audio-vorbis/effect-356"),
        ("audiostreams", 2, "audio-vorbis/song-2"),
    ] {
        let bytes = pack
            .read_group(archive, group)
            .expect("group")
            .remove(&0)
            .expect("file 0");
        let sound = SoundFile::parse(&bytes).expect("sound header");
        let chunks = chunks_of(&pack, archive, &bytes, &sound).expect("chunks");
        let mut pcm = Vec::new();
        decode_with((sound.loop_start, sound.loop_end), &chunks, 0, |bytes| {
            pcm.extend_from_slice(bytes)
        });
        rs910_core::test_support::frozen::assert_stream(name, &pcm);
    }
}

/// A synthetic stereo stream whose mappings use two submaps (`mux` [0, 1] with
/// channel coupling, and [1, 0] without), the original client's PCM recorded
/// once over the same bytes. No sound in the shipped cache has more than one
/// submap, so this stream is the only proof of the shared-window behaviour the
/// decoder keeps for them (see `rs910_audio::audio_vorbis`). It needs no
/// cache.
#[test]
fn multi_submap_stream_matches_the_original_decoder() {
    let stream = rs910_core::test_support::frozen::bytes("audio-vorbis/submaps.ogg");
    let mut pcm = Vec::new();
    let frames = decode_with((0, 0), &[stream], 0, |bytes| pcm.extend_from_slice(bytes));
    assert_eq!(frames, 7040, "the stream's final granule position");
    rs910_core::test_support::frozen::assert_stream("audio-vorbis/submaps", &pcm);
}
